//! Strictly allowlisted mutations of market-analysis UI state.
//! No generic command interpreter is exposed to external processes.
#![cfg(unix)]

use serde_json::{Value, json};
use tr_control::{Command, Reply, valid_expression, valid_symbol};

use super::TerminalApp;

impl TerminalApp {
    pub(super) fn poll_ipc(&mut self) {
        let Some(receiver) = self.ipc.as_ref() else { return };
        let mut pending = Vec::new();
        // A malicious or buggy CLI cannot consume an unbounded GUI frame.
        for _ in 0..32 {
            match receiver.rx.try_recv() {
                Ok(request) => pending.push(request),
                Err(_) => break,
            }
        }
        for request in pending {
            let result = self.handle_ipc(request.command);
            let _ = request.response.send(result);
        }
    }

    fn handle_ipc(&mut self, cmd: Command) -> Reply {
        match self.try_handle_ipc(cmd) {
            Ok(data) => Reply::success(data),
            Err(error) => Reply::failure(error),
        }
    }

    fn try_handle_ipc(&mut self, cmd: Command) -> Result<Value, String> {
        let symbol_key = |input: &str| -> Result<String, String> {
            if !valid_symbol(input) {
                return Err("invalid symbol: use a market key of up to 96 ordinary characters".into());
            }
            Ok(tr_data::security(input).key())
        };
        match cmd {
            Command::Status => Ok(json!({
                "running": true,
                "pid": std::process::id(),
                "provider": self.provider,
                "layout": self.layout_name,
                "panes": self.panes.len(),
                "watchlist_count": self.all_watchlist_symbol_count(),
                "base_watchlist_count": self.watchlist.len(),
                "watchlist_group_count": self.watchlist_groups.len(),
                "price_alert_count": self.price_alerts.len(),
                "technical_alert_count": self.technical_alerts.len(),
                "status": self.status,
                "read_only": true,
                "trade_execution": false,
            })),
            Command::Watchlist => Ok(json!({
                "symbols": self.watchlist,
                "groups": self.watchlist_groups,
                "unique_symbol_count": self.all_watchlist_symbol_count(),
            })),
            Command::WatchGroupsImport { groups } => {
                if groups.len() > 30 {
                    return Err("no more than 30 watchlist groups are allowed".into());
                }
                let mut group_names = std::collections::HashSet::new();
                let mut normalized = Vec::with_capacity(groups.len());
                let mut total = 0;
                for group in groups {
                    let name = group.name.trim();
                    if name.is_empty()
                        || name.len() > 80
                        || name.chars().any(char::is_control)
                        || !group_names.insert(name.to_owned())
                    {
                        return Err("invalid or duplicate watchlist group name".into());
                    }
                    if group.symbols.len() > 90 {
                        return Err(format!("group {name} exceeds 90 symbols"));
                    }
                    let mut seen = std::collections::HashSet::new();
                    let mut symbols = Vec::new();
                    for symbol in group.symbols {
                        let key = symbol_key(&symbol)?;
                        let sec = tr_data::security(&key);
                        if !tr_data::kiwoom::is_domestic(&sec) {
                            return Err(format!("group {name} contains a non-Korean security"));
                        }
                        if seen.insert(key.clone()) {
                            symbols.push(key);
                        }
                    }
                    total += symbols.len();
                    if total > 500 {
                        return Err("import contains more than 500 group entries".into());
                    }
                    normalized.push(tr_control::WatchlistGroup {
                        name: name.into(),
                        symbols,
                        collapsed: group.collapsed,
                    });
                }
                self.watchlist_groups = normalized;
                self.watchlist_pane_group = None;
                self.quote_batch_cursor = 0;
                self.last_quotes = None;
                self.save_settings();
                Ok(json!({
                    "group_count":self.watchlist_groups.len(),
                    "group_entries":total,
                    "base_watchlist_count":self.watchlist.len(),
                    "unique_symbol_count":self.all_watchlist_symbol_count(),
                }))
            }
            Command::WatchAdd { symbol } => {
                let key = symbol_key(&symbol)?;
                if self.watchlist.len() >= 500 && !self.watchlist.contains(&key) {
                    return Err("watchlist limit is 500 symbols".into());
                }
                if !self.watchlist.contains(&key) {
                    self.watchlist.push(key.clone());
                    self.request_security_names(vec![key.clone()]);
                    self.last_quotes = None;
                    self.last_technicals = None;
                    self.save_settings();
                }
                Ok(json!({"symbol":key,"symbols":self.watchlist}))
            }
            Command::WatchRemove { symbol } => {
                let key = symbol_key(&symbol)?;
                self.watchlist.retain(|s| s != &key);
                self.save_settings();
                Ok(json!({"removed":key,"symbols":self.watchlist}))
            }
            Command::Screen { expression } => {
                if !valid_expression(&expression) {
                    return Err("screen expression must be 1–256 printable characters".into());
                }
                let before = self.screen.clone();
                self.screen_command(&expression);
                if self.status.starts_with("SCAN [") || self.status.starts_with("SCAN ALL |") {
                    self.screen = before;
                    return Err(self.status.clone());
                }
                self.save_settings();
                Ok(json!({"status":self.status,"screen":self.screen}))
            }
            Command::PriceAlert { expression } => {
                if !valid_expression(&expression) {
                    return Err("price alert expression must be printable".into());
                }
                let valid = expression.eq_ignore_ascii_case("list")
                    || expression.eq_ignore_ascii_case("clear")
                    || crate::screening::parse_alert_spec(&expression).is_some();
                if !valid {
                    return Err("expected SYMBOL ABOVE|BELOW PRICE, LIST or CLEAR".into());
                }
                self.price_alert_command(&expression);
                self.save_settings();
                Ok(json!({"status":self.status, "alerts":self.price_alerts}))
            }
            Command::TechnicalAlert { expression } => {
                if !valid_expression(&expression) {
                    return Err("technical alert expression must be printable".into());
                }
                let valid = expression.eq_ignore_ascii_case("list")
                    || expression.eq_ignore_ascii_case("clear")
                    || crate::screening::parse_technical_alert_spec(&expression).is_some();
                if !valid {
                    return Err("expected SYMBOL VOL|RSI|SMA condition, LIST or CLEAR".into());
                }
                self.technical_alert_command(&expression);
                self.save_settings();
                Ok(json!({"status":self.status, "alerts":self.technical_alerts}))
            }
            Command::OpenChart { symbol } => {
                let key = symbol_key(&symbol)?;
                self.open_security(&key);
                Ok(json!({"symbol":key, "status":self.status}))
            }
            Command::Quotes { symbols } => {
                if symbols.is_empty() || symbols.len() > 50 {
                    return Err("request 1–50 symbols".into());
                }
                let keys: Vec<String> = symbols.iter().map(|s| symbol_key(s)).collect::<Result<_, _>>()?;
                let quotes: Vec<_> = keys.iter().filter_map(|key| self.quotes.get(key)).collect();
                Ok(json!({"requested":keys,"quotes":quotes,"source":"in-app cached observations"}))
            }
        }
    }
}

#[cfg(test)]
mod grouped_watchlist_tests {
    #[test]
    fn screenshot_symbols_are_canonical_domestic_market_keys() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../../docs/kiwoom-screenshot-watchlists-resolved-2026-11-05.json"))
                .unwrap();
        for group in manifest["groups"].as_array().unwrap() {
            for symbol in group["symbols"].as_array().unwrap() {
                let code = symbol.as_str().unwrap();
                let sec = tr_data::security(&tr_data::security(code).key());
                assert!(
                    tr_data::kiwoom::is_domestic(&sec),
                    "group {} invalid domestic key {code}, parsed as {sec:?}",
                    group["name"]
                );
            }
        }
    }
}
