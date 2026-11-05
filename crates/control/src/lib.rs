//! Versioned, allowlisted, user-local IPC protocol for Terminal-rs.
//! No order placement, cancellation, trade signing, shell or file editing.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

pub const MAX_MESSAGE_BYTES: usize = 16 * 1024;
pub const SOCKET_NAME: &str = "terminal-control.sock";

/// A named watchlist group. A symbol can appear in several groups, but
/// market-data subscriptions are deduplicated by the GUI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatchlistGroup {
    pub name: String,
    pub symbols: Vec<String>,
    #[serde(default = "default_collapsed")]
    pub collapsed: bool,
}

fn default_collapsed() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Status,
    Watchlist,
    WatchGroupsImport { groups: Vec<WatchlistGroup> },
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

    #[test]
    fn grouped_watchlist_command_preserves_symbols_and_unicode_names() {
        let c = Command::WatchGroupsImport {
            groups: vec![WatchlistGroup {
                name: "반도체기술".into(),
                symbols: vec!["005930.KS US Equity".into(), "00088K.KS US Equity".into()],
                collapsed: true,
            }],
        };
        let encoded = serde_json::to_vec(&c).unwrap();
        assert!(encoded.len() < MAX_MESSAGE_BYTES);
        let restored: Command = serde_json::from_slice(&encoded).unwrap();
        match restored {
            Command::WatchGroupsImport { groups } => {
                assert_eq!(groups[0].name, "반도체기술");
                assert_eq!(groups[0].symbols.len(), 2);
                assert!(groups[0].collapsed);
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn screenshot_manifest_has_fourteen_valid_groups_and_no_duplicate_entries() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../../docs/kiwoom-screenshot-watchlists-resolved-2026-11-05.json"))
                .unwrap();
        let groups: Vec<WatchlistGroup> = serde_json::from_value(manifest["groups"].clone()).unwrap();
        assert_eq!(groups.len(), 14);
        assert_eq!(groups.iter().map(|g| g.symbols.len()).sum::<usize>(), 147);
        for group in &groups {
            let symbols: std::collections::HashSet<_> = group.symbols.iter().collect();
            assert_eq!(symbols.len(), group.symbols.len(), "duplicate within group {}", group.name);
            for symbol in &group.symbols {
                assert!(valid_symbol(symbol), "{}: {symbol}", group.name);
                assert!(symbol.ends_with(".KS") || symbol.ends_with(".KQ"));
            }
        }
        let all: std::collections::HashSet<_> = groups.iter().flat_map(|g| &g.symbols).collect();
        assert_eq!(all.len(), 140);
    }
}
