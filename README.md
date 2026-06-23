# 🧠 logmachine (Rust)

Rust re-implementation of the LogMachine mentality used in the Go and Python libraries.

This crate installs a **global logger** (`log` crate backend), so logs from the whole Rust app flow through LogMachine formatting and file capture.

## Usage

```rust
use logmachine::{init_global_logger, success, LogMachineOptions, CentralConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_global_logger(LogMachineOptions {
        debug_level: 0,
        ..Default::default()
    })?;

    log::info!("Hello from info");
    log::warn!("Hello from warn");
    log::error!("Hello from error");
    success("Hello from success");
    Ok(())
}
```

### With central logging (HTTP)

```rust
use std::collections::HashMap;

init_global_logger(LogMachineOptions {
    central: Some(CentralConfig {
        url: "https://logmachine.org".to_string(),
        room: "public".to_string(),
        endpoint: "/api/logs".to_string(),
        headers: HashMap::new(),
        socketio: false,
        socketio_path: "/api/socket.io/".to_string(),
    }),
    ..Default::default()
})?;
```

## Behavior parity

- Uses process-global logging (`log::set_logger`) to capture logs from the app.
- Writes all logs to `logs.log`.
- Writes only error-level logs to `errors.log`.
- Applies debug-level filtering only to console output (not file writes), matching Go/Python behavior.
- Keeps LogMachine formatted output including timing markers and emojis.

## Auth & credentials

- The Rust SDK now supports simple credential persistence and automatic auth header merging to match the Python and Go SDKs.
- Credentials are stored in a plain `KEY=VALUE` file at `~/.logmachine`. Supported keys:
    - `lm_username` / `CL_USERNAME` — persisted username used by formatters
    - `lm_auth_token` — bearer token used for central requests
    - `lm_expiry` — RFC3339 token expiry (optional)
- When a token is present in `~/.logmachine` (or `process env`), the SDK will automatically add an `Authorization: Bearer <token>` header to central HTTP requests unless an `Authorization` header is already set in `CentralConfig.headers`.
- Simple helpers available in the crate:
    - `login_with_api_key(central: &CentralConfig, api_key: &str)` — persist an API key as the token and attempt a best-effort session sync to populate `lm_username`.
    - `logout()` — clear persisted credentials.

Notes:
- Device-flow authentication is not implemented in the Rust crate yet; the helpers focus on API-key persistence and session username sync (parity with other SDKs).
- To use these features, call `login_with_api_key` before emitting logs (or rely on the SDK to load existing `~/.logmachine` entries on init).
