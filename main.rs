use chrono::Local;
use log::{Level, LevelFilter, Log, Metadata, Record};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::collections::HashMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

const COLOR_DEBUG: &str = "\x1b[36m";
const COLOR_INFO: &str = "\x1b[34m";
const COLOR_WARNING: &str = "\x1b[33m";
const COLOR_ERROR: &str = "\x1b[31m";
const COLOR_SUCCESS: &str = "\x1b[32m";
const COLOR_RESET: &str = "\x1b[0m";
const COLOR_BOLD: &str = "\x1b[1m";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogEntry {
    pub user: String,
    pub module: String,
    pub level: String,
    pub timestamp: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct LogMachineOptions {
    pub log_file: String,
    pub error_file: String,
    pub debug_level: u8,
    pub verbose: bool,
    pub central: Option<CentralConfig>,
    pub attached: bool,
}

#[derive(Debug, Clone)]
pub struct CentralConfig {
    pub url: String,
    pub room: String,
    pub endpoint: String,
    pub headers: HashMap<String, String>,
    pub socketio: bool,
    pub socketio_path: String,
}

impl Default for LogMachineOptions {
    fn default() -> Self {
        Self {
            log_file: "logs.log".to_string(),
            error_file: "errors.log".to_string(),
            debug_level: 0,
            verbose: false,
            central: None,
            attached: false,
        }
    }
}

#[derive(Debug)]
pub enum LogMachineError {
    Io(std::io::Error),
    SetLogger(log::SetLoggerError),
    Http(reqwest::Error),
    Config(String),
}

impl std::fmt::Display for LogMachineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::SetLogger(err) => write!(f, "failed to set global logger: {err}"),
            Self::Http(err) => write!(f, "http error: {err}"),
            Self::Config(err) => write!(f, "config error: {err}"),
        }
    }
}

impl std::error::Error for LogMachineError {}

impl From<std::io::Error> for LogMachineError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<log::SetLoggerError> for LogMachineError {
    fn from(value: log::SetLoggerError) -> Self {
        Self::SetLogger(value)
    }
}

impl From<reqwest::Error> for LogMachineError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

#[derive(Debug)]
struct LogMachineState {
    debug_level: u8,
    verbose: bool,
    central: Option<CentralConfig>,
    central_client: Option<reqwest::blocking::Client>,
    attached: bool,
    allowed_map: HashMap<u8, Vec<String>>,
    log_file: File,
    error_file: File,
}

impl LogMachineState {
    fn new(options: LogMachineOptions) -> Result<Self, LogMachineError> {
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&options.log_file)?;
        let error_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&options.error_file)?;

        let mut allowed_map = HashMap::new();
        allowed_map.insert(1, vec!["ERROR".to_string()]);
        allowed_map.insert(2, vec!["SUCCESS".to_string()]);
        allowed_map.insert(3, vec!["WARN".to_string()]);
        allowed_map.insert(4, vec!["INFO".to_string()]);
        allowed_map.insert(5, vec!["ERROR".to_string(), "WARN".to_string()]);
        allowed_map.insert(6, vec!["INFO".to_string(), "SUCCESS".to_string()]);
        allowed_map.insert(
            7,
            vec!["ERROR".to_string(), "WARN".to_string(), "INFO".to_string()],
        );

        let central_client = if options.central.is_some() {
            Some(build_central_client(Duration::from_secs(10)).build()?)
        } else {
            None
        };

        Ok(Self {
            debug_level: options.debug_level,
            verbose: options.verbose,
            central: options.central,
            central_client,
            attached: options.attached,
            allowed_map,
            log_file,
            error_file,
        })
    }

    fn level_name(level: Level, message: &str) -> &'static str {
        if level == Level::Info && message.starts_with("SUCCESS:") {
            "SUCCESS"
        } else {
            match level {
                Level::Error => "ERROR",
                Level::Warn => "WARN",
                Level::Info => "INFO",
                Level::Debug | Level::Trace => "DEBUG",
            }
        }
    }

    fn level_color(level_name: &str) -> &'static str {
        match level_name {
            "DEBUG" => COLOR_DEBUG,
            "INFO" => COLOR_INFO,
            "WARN" => COLOR_WARNING,
            "ERROR" => COLOR_ERROR,
            "SUCCESS" => COLOR_SUCCESS,
            _ => COLOR_INFO,
        }
    }

    fn is_allowed(&self, level_name: &str) -> bool {
        if self.debug_level == 0 || self.verbose {
            return true;
        }

        if let Some(allowed) = self.allowed_map.get(&self.debug_level) {
            return allowed.iter().any(|l| l == level_name);
        }
        true
    }

    fn format_log(&self, level_name: &str, message: &str, module_path: Option<&str>) -> String {
        let username = env::var("CL_USERNAME").unwrap_or_else(|_| get_login());
        let timestamp = Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string();
        let parent_dir = module_path
            .and_then(|path| Path::new(path).parent())
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("stdin");

        let color = Self::level_color(level_name);
        let level_fmt = format!("{COLOR_BOLD}{color}[ {level_name} ]{COLOR_RESET}");

        format!(
            "{COLOR_DEBUG}({username}{COLOR_RESET} @ {COLOR_WARNING}{parent_dir}{COLOR_RESET}) 🤌 CL Timing: {color}[ {timestamp} ]{COLOR_RESET}\n{level_fmt} {message}\n🏁"
        )
    }

    fn write_log_entry(
        &mut self,
        level_name: &str,
        message: &str,
        module_path: Option<&str>,
    ) -> std::io::Result<String> {
        let formatted = self.format_log(level_name, message, module_path);
        writeln!(self.log_file, "{formatted}")?;
        self.log_file.flush()?;

        if level_name == "ERROR" {
            writeln!(self.error_file, "{formatted}")?;
            self.error_file.flush()?;
        }

        if self.is_allowed(level_name) {
            println!("{formatted}");
        }
        Ok(formatted)
    }
}

#[derive(Debug)]
struct LogMachineGlobalLogger;
thread_local! {
    static IN_CENTRAL_EMIT: Cell<bool> = const { Cell::new(false) };
}

impl Log for LogMachineGlobalLogger {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let target = record.target();
        if target.starts_with("reqwest")
            || target.starts_with("hyper")
            || target.starts_with("h2")
            || target.starts_with("tokio")
            || target.starts_with("rustls")
            || target.starts_with("mio")
            || target.starts_with("want")
            || target.starts_with("tower")
        {
            return;
        }

        if let Ok(guard) = LOGGER_STATE.read() {
            if let Some(shared_state) = guard.as_ref() {
                if let Ok(mut state) = shared_state.lock() {
                    let raw_message = record.args().to_string();
                    let normalized_message = raw_message
                        .strip_prefix("SUCCESS:")
                        .map(|m| m.trim_start())
                        .unwrap_or(raw_message.as_str());
                    let level_name = LogMachineState::level_name(record.level(), &raw_message);
                    let central = state.central.clone();
                    let central_client = state.central_client.clone();
                    let attached = state.attached;
                    if let Ok(formatted) = state.write_log_entry(level_name, normalized_message, record.file()) {
                        drop(state);
                        if let Err(err) = emit_central(central.as_ref(), central_client.as_ref(), attached, &formatted) {
                            eprintln!("[logmachine] transport error: {err}");
                        }
                    }
                }
            }
        }
    }

    fn flush(&self) {
        if let Ok(guard) = LOGGER_STATE.read() {
            if let Some(shared_state) = guard.as_ref() {
                if let Ok(mut state) = shared_state.lock() {
                    let _ = state.log_file.flush();
                    let _ = state.error_file.flush();
                }
            }
        }
    }
}

static LOGGER_STATE: Lazy<RwLock<Option<Arc<Mutex<LogMachineState>>>>> = Lazy::new(|| RwLock::new(None));

fn emit_central(
    central: Option<&CentralConfig>,
    central_client: Option<&reqwest::blocking::Client>,
    attached: bool,
    formatted: &str,
) -> Result<(), LogMachineError> {
    let Some(central) = central else {
        return Ok(());
    };

    if central.room.trim().is_empty() {
        return Err(LogMachineError::Config(
            "central config must include 'room' for log transport".to_string(),
        ));
    }

    let Some(log_data) = parse_log(formatted) else {
        return Ok(());
    };

    let endpoint = if central.endpoint.is_empty() {
        "/api/logs"
    } else {
        central.endpoint.as_str()
    };

    let mut url = build_central_url(central.url.as_str(), endpoint)?;
    url.query_pairs_mut().append_pair("room", central.room.as_str());

    let _socketio_requested = attached || central.socketio;
    let _socketio_path = &central.socketio_path;

    let mut should_emit = true;
    IN_CENTRAL_EMIT.with(|flag| {
        if flag.get() {
            should_emit = false;
        } else {
            flag.set(true);
        }
    });
    if !should_emit {
        return Ok(());
    }

    let result = (|| -> Result<(), LogMachineError> {
        let client = central_client.ok_or_else(|| {
            LogMachineError::Config("missing central HTTP client".to_string())
        })?;
        let mut request = client.post(url).json(&log_data);
        for (key, value) in &central.headers {
            request = request.header(key, value);
        }

        let response = request.send()?;
        if !response.status().is_success() {
            return Err(LogMachineError::Config(format!(
                "failed to send log to central: {}",
                response.status()
            )));
        }
        Ok(())
    })();

    IN_CENTRAL_EMIT.with(|flag| flag.set(false));
    result
}

pub fn init_global_logger(options: LogMachineOptions) -> Result<(), LogMachineError> {
    if let Some(central) = &options.central {
        resolve_username(&central.url);
    }

    let new_state = Arc::new(Mutex::new(LogMachineState::new(options)?));
    let mut guard = LOGGER_STATE
        .write()
        .map_err(|_| LogMachineError::Config("failed to acquire logger state lock".to_string()))?;
    if guard.is_none() {
        let logger = LogMachineGlobalLogger;
        log::set_boxed_logger(Box::new(logger))?;
        log::set_max_level(LevelFilter::Trace);
    }
    *guard = Some(new_state);
    Ok(())
}

pub fn success(message: &str) {
    log::info!("SUCCESS: {message}");
}

pub fn parse_log(log_text: &str) -> Option<LogEntry> {
    static ANSI_ESCAPE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\x1b\[[0-9;]*m").expect("valid ansi regex"));
    static HEADER_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\((.*?) @ (.*?)\) 🤌 CL Timing: \[ (.*?) \]").expect("valid header regex"));
    static LEVEL_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\[\s?(\w+)\s?\]\s?(.*)").expect("valid level regex"));

    let clean = ANSI_ESCAPE.replace_all(log_text.trim(), "");
    let header = HEADER_RE.captures(&clean)?;
    let user = header.get(1)?.as_str().to_string();
    let module = header.get(2)?.as_str().to_string();
    let timestamp = header.get(3)?.as_str().to_string();

    let mut lines = clean.lines();
    let _ = lines.next();
    let level_line = lines.next().unwrap_or_default().trim();
    let level_caps = LEVEL_RE.captures(level_line);

    let (level, message) = if let Some(caps) = level_caps {
        let lvl = caps
            .get(1)
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_else(|| "UNKNOWN".to_string());
        let msg = caps
            .get(2)
            .map(|m| m.as_str().replace('🏁', "").trim().to_string())
            .unwrap_or_default();
        (lvl, msg)
    } else {
        ("UNKNOWN".to_string(), String::new())
    };

    Some(LogEntry {
        user,
        module,
        level,
        timestamp,
        message,
    })
}

pub fn jsonifier(log_file_path: &str) -> Result<Vec<String>, LogMachineError> {
    let content = fs::read_to_string(log_file_path)?;
    let mut entries = Vec::new();

    for block in content.split('🏁') {
        let trimmed = block.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        let normalized = format!("{trimmed}\n🏁");
        if let Some(entry) = parse_log(&normalized) {
            if let Ok(json) = serde_json::to_string(&entry) {
                entries.push(json);
            }
        }
    }

    Ok(entries)
}

fn get_login() -> String {
    env::var("USER")
        .or_else(|_| env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn resolve_username(server_url: &str) {
    let cl_file = Path::new(&home_dir()).join(".cl_username");
    if let Ok(data) = fs::read_to_string(&cl_file) {
        env::set_var("CL_USERNAME", data.trim());
        return;
    }

    let login = get_login();
    let mut url = match build_central_url(server_url, "/api/get_username") {
        Ok(url) => url,
        Err(_) => {
            env::set_var("CL_USERNAME", "unknown");
            return;
        }
    };
    url.query_pairs_mut().append_pair("base", login.as_str());

    let username = build_central_client(Duration::from_secs(5))
        .build()
        .ok()
        .and_then(|client| client.get(url).send().ok())
        .filter(|resp| resp.status().is_success())
        .and_then(|resp| resp.json::<UsernameResponse>().ok())
        .map(|r| r.username)
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    env::set_var("CL_USERNAME", &username);
    if username != "unknown" {
        let _ = write_username_cache(&cl_file, &username);
    }
}

fn home_dir() -> String {
    env::var("HOME").unwrap_or_else(|_| ".".to_string())
}

#[derive(Debug, Deserialize)]
struct UsernameResponse {
    username: String,
}

fn build_central_client(timeout: Duration) -> reqwest::blocking::ClientBuilder {
    reqwest::blocking::Client::builder().timeout(timeout)
}

fn build_central_url(base_url: &str, endpoint: &str) -> Result<reqwest::Url, LogMachineError> {
    let endpoint_url = if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint.to_string()
    } else {
        format!("{}{}", base_url.trim_end_matches('/'), endpoint)
    };
    reqwest::Url::parse(endpoint_url.as_str()).map_err(|err| {
        LogMachineError::Config(format!("invalid central URL '{endpoint_url}': {err}"))
    })
}

fn write_username_cache(path: &Path, username: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)?;

    #[cfg(not(unix))]
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;

    file.write_all(username.as_bytes())?;
    file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_cell::sync::Lazy;
    use std::net::TcpListener;
    use std::io::Read;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};
    static TEST_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    fn test_guard() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn tmp_file(name: &str) -> String {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock ok")
            .as_nanos();
        format!("/tmp/{name}_{ts}.log")
    }

    fn read_file(path: &str) -> String {
        fs::read_to_string(path).unwrap_or_default()
    }

    fn test_options() -> LogMachineOptions {
        LogMachineOptions {
            log_file: tmp_file("logmachine_logs"),
            error_file: tmp_file("logmachine_errors"),
            debug_level: 0,
            verbose: false,
            central: None,
            attached: false,
        }
    }

    #[test]
    fn parse_log_extracts_fields() {
        let _guard = test_guard();
        let sample = "\u{1b}[36m(testuser\u{1b}[0m @ \u{1b}[33mapp\u{1b}[0m) 🤌 CL Timing: \u{1b}[34m[ 2026-04-01T10:00:00+00:00 ]\u{1b}[0m\n\u{1b}[1m\u{1b}[34m[ INFO ]\u{1b}[0m hello world\n🏁";
        let parsed = parse_log(sample).expect("should parse");
        assert_eq!(parsed.user, "testuser");
        assert_eq!(parsed.module, "app");
        assert_eq!(parsed.level, "INFO");
        assert_eq!(parsed.message, "hello world");
    }

    #[test]
    fn global_logger_captures_rust_log_macros() {
        let _guard = test_guard();
        let opts = test_options();
        let log_path = opts.log_file.clone();
        let err_path = opts.error_file.clone();
        init_global_logger(opts).expect("init logger");

        log::info!("hello from info");
        success("hello from success");
        log::error!("hello from error");
        log::logger().flush();

        let log_data = read_file(&log_path);
        assert!(log_data.contains("hello from info"));
        assert!(log_data.contains("hello from success"));
        assert!(log_data.contains("[ SUCCESS ]"));
        assert!(log_data.contains("hello from error"));

        let err_data = read_file(&err_path);
        assert!(err_data.contains("hello from error"));
        assert!(!err_data.contains("hello from info"));
    }

    #[test]
    fn jsonifier_returns_entries() {
        let _guard = test_guard();
        let opts = test_options();
        let log_path = opts.log_file.clone();
        init_global_logger(opts).expect("init logger");

        log::warn!("entry one");
        log::info!("entry two");
        log::logger().flush();

        let entries = jsonifier(&log_path).expect("jsonifier");
        assert!(entries.len() >= 2);
        assert!(entries.iter().any(|e| e.contains("\"message\":\"entry one\"")));
    }

    #[test]
    fn central_http_transport_sends_logs() {
        let _guard = test_guard();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        listener
            .set_nonblocking(true)
            .expect("set nonblocking");

        let server = thread::spawn(move || {
            let mut saw_post = String::new();
            let start = std::time::Instant::now();
            while start.elapsed() < Duration::from_secs(5) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .expect("set timeout");

                        let req = read_http_request(&mut stream);
                        if req.starts_with("GET /api/get_username") {
                            let body = r#"{"username":"mockuser"}"#;
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                body.len(),
                                body
                            );
                            let _ = stream.write_all(response.as_bytes());
                            continue;
                        }

                        if req.starts_with("POST /api/logs?room=my_room") {
                            saw_post = req;
                            let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
                            let _ = stream.write_all(response.as_bytes());
                            break;
                        }
                    }
                    Err(_) => {
                        thread::sleep(Duration::from_millis(20));
                    }
                }
            }
            saw_post
        });

        let mut opts = test_options();
        opts.central = Some(CentralConfig {
            url: format!("http://{}", addr),
            room: "my_room".to_string(),
            endpoint: "/api/logs".to_string(),
            headers: HashMap::from([("X-Test".to_string(), "token".to_string())]),
            socketio: false,
            socketio_path: "/api/socket.io/".to_string(),
        });
        init_global_logger(opts).expect("init logger");

        log::info!("central payload");
        log::logger().flush();

        let post_request = server.join().expect("join server");
        assert!(post_request.contains("POST /api/logs?room=my_room"));
        let lower = post_request.to_lowercase();
        assert!(lower.contains("application/json"));
        assert!(lower.contains("x-test: token"));
        assert!(post_request.contains("central payload"));
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        let mut header_buf = Vec::new();
        let mut chunk = [0_u8; 1024];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    header_buf.extend_from_slice(&chunk[..n]);
                    if let Some(headers_end) = header_buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header_end_idx = headers_end + 4;
                        let header_text = String::from_utf8_lossy(&header_buf[..header_end_idx]).to_string();
                        let content_length = header_text
                            .lines()
                            .find_map(|line| {
                                let mut parts = line.splitn(2, ':');
                                let key = parts.next()?.trim();
                                let val = parts.next()?.trim();
                                if key.eq_ignore_ascii_case("Content-Length") {
                                    val.parse::<usize>().ok()
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(0);

                        let body_already = header_buf.len().saturating_sub(header_end_idx);
                        let remaining = content_length.saturating_sub(body_already);
                        if remaining == 0 {
                            break;
                        }

                        let mut body_tail = vec![0_u8; remaining];
                        if stream.read_exact(&mut body_tail).is_ok() {
                            header_buf.extend_from_slice(&body_tail);
                        }
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        String::from_utf8_lossy(&header_buf).to_string()
    }
}
