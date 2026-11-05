//! Versioned, allowlisted, user-local IPC protocol for Terminal-rs.
//! No order placement, cancellation, trade signing, shell or file editing.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

pub const MAX_MESSAGE_BYTES: usize = 16 * 1024;
pub const SOCKET_NAME: &str = "terminal-control.sock";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Status,
    Watchlist,
    WatchAdd { symbol: String },
    WatchRemove { symbol: String },
    Screen { expression: String },
    PriceAlert { expression: String },
    TechnicalAlert { expression: String },
    OpenChart { symbol: String },
    Quotes { symbols: Vec<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reply {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Reply {
    pub fn success(data: Value) -> Self {
        Self { ok: true, data: Some(data), error: None }
    }
    pub fn failure(error: impl Into<String>) -> Self {
        Self { ok: false, data: None, error: Some(error.into()) }
    }
}

pub fn socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("TR_DATA_DIR") {
        return PathBuf::from(path).join(SOCKET_NAME);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/terminal").join(SOCKET_NAME)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("terminal")
            .join(SOCKET_NAME)
    }
}

pub fn valid_symbol(input: &str) -> bool {
    let s = input.trim();
    !s.is_empty()
        && s.len() <= 96
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '^' | ':' | '-' | '/'))
}

pub fn valid_expression(input: &str) -> bool {
    !input.is_empty() && input.len() <= 256 && !input.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_rejects_trades_and_unexpected_fields() {
        for cmd in [
            r#"{"action":"buy","symbol":"AAPL"}"#,
            r#"{"action":"cancel_order","id":"abc"}"#,
            r#"{"action":"watch_add","symbol":"AAPL","command":"BUY"}"#,
        ] {
            assert!(serde_json::from_str::<Command>(cmd).is_err(), "{cmd}");
        }
        assert!(!valid_symbol("AAPL; rm -rf ."));
        assert!(!valid_expression("SCAN\nBUY"));
    }

    #[test]
    fn replies_round_trip() {
        let r = Reply::success(serde_json::json!({"read_only":true}));
        let out: Reply = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert!(out.ok && out.data.unwrap()["read_only"] == true);
    }
}
