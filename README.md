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
        url: "https://logmachine.bufferpunk.com".to_string(),
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
