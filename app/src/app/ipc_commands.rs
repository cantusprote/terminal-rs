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
                "watchlist_count": self.watchlist.len(),
                "price_alert_count": self.price_alerts.len(),
                "technical_alert_count": self.technical_alerts.len(),
                "status": self.status,
                "read_only": true,
                "trade_execution": false,
            })),
            Command::Watchlist => Ok(json!({"symbols": self.watchlist})),
            Command::WatchAdd { symbol } => {
                let key = symbol_key(&symbol)?;
                if self.watchlist.len() >= 500 && !self.watchlist.contains(&key) {
                    return Err("watchlist limit is 500 symbols".into());
                }
                if !self.watchlist.contains(&key) {
                    self.watchlist.push(key.clone());
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
