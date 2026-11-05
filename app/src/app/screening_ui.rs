//! Watchlist screening commands, local price alerts and observation processing.
//! Commands only change presentation and local notifications; never orders.

use tr_data::Quote;

use super::{PaneKind, TerminalApp, merge_quotes};
use crate::screening::{
    Direction, PriceAlert, ScreenMode, ScreenSort, TechnicalAlert, TechnicalSnapshot, parse_alert_spec,
    parse_technical_alert_spec,
};

impl TerminalApp {
    pub(super) fn screen_command(&mut self, arg: &str) {
        let words: Vec<&str> = arg.split_whitespace().collect();
        let mode = words.first().map(|s| s.to_ascii_uppercase()).unwrap_or_default();
        let amount = || words.get(1)?.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0 && *v <= 1000.0);
        let mut valid = true;
        let mut next = self.screen.clone();
        match mode.as_str() {
            "" => {}
            "ALL" | "RESET" => next = Default::default(),
            "UP" | "GAINERS" | "DOWN" | "LOSERS" | "MOVE" | "MOVERS" => {
                next.mode = match mode.as_str() {
                    "UP" | "GAINERS" => ScreenMode::Gainers,
                    "DOWN" | "LOSERS" => ScreenMode::Losers,
                    _ => ScreenMode::Movers,
                };
                if words.len() > 1 {
                    if let Some(n) = amount() {
                        next.min_move_pct = n;
                    } else {
                        valid = false;
                    }
                }
            }
            "VOL" | "VOLUME" => {
                next.mode = ScreenMode::VolumeSurge;
                if words.len() > 1 {
                    match amount().filter(|n| *n > 0.0) {
                        Some(n) => next.min_volume_ratio = n,
                        None => valid = false,
                    }
                }
                self.last_technicals = None;
            }
            "SMA" | "SMA20" => {
                next.mode = match words.get(1).map(|s| s.to_ascii_uppercase()) {
                    Some(v) if v == "ABOVE" || v == "UP" => ScreenMode::AboveSma20,
                    Some(v) if v == "BELOW" || v == "DOWN" => ScreenMode::BelowSma20,
                    _ => {
                        valid = false;
                        ScreenMode::All
                    }
                };
                self.last_technicals = None;
            }
            "RSI" | "RSI14" => {
                next.mode = match words.get(1).map(|s| s.to_ascii_uppercase()) {
                    Some(v) if v == "HIGH" || v == "ABOVE" => ScreenMode::RsiHigh,
                    Some(v) if v == "LOW" || v == "BELOW" => ScreenMode::RsiLow,
                    _ => {
                        valid = false;
                        ScreenMode::All
                    }
                };
                self.last_technicals = None;
            }
            "FRESH" => match words.get(1).map(|s| s.to_ascii_uppercase()) {
                Some(v) if v == "OFF" || v == "ALL" => next.max_age_minutes = None,
                Some(v) => match v.parse::<u32>().ok().filter(|v| (1..=1440).contains(v)) {
                    Some(n) => next.max_age_minutes = Some(n),
                    None => valid = false,
                },
                None => valid = false,
            },
            "SORT" => match words.get(1).map(|s| s.to_ascii_uppercase()) {
                Some(v) if v == "UP" || v == "GAINERS" => next.sort = ScreenSort::Gainers,
                Some(v) if v == "DOWN" || v == "LOSERS" => next.sort = ScreenSort::Losers,
                Some(v) if v == "SYMBOL" || v == "NAME" => next.sort = ScreenSort::Symbol,
                Some(v) if v == "WATCHLIST" || v == "DEFAULT" => next.sort = ScreenSort::Watchlist,
                _ => valid = false,
            },
            _ => valid = false,
        }
        if !valid {
            self.status =
                "SCAN ALL | UP/DOWN/MOVE % | VOL 2 | SMA ABOVE/BELOW | RSI HIGH/LOW | FRESH 30 | SORT GAINERS".into();
            return;
        }
        self.screen = next;
        let threshold = if self.screen.mode == ScreenMode::VolumeSurge {
            format!("volume ≥ {:.1}× the prior 20-day average", self.screen.min_volume_ratio)
        } else {
            format!("minimum move {:.1}%", self.screen.min_move_pct)
        };
        self.status = format!(
            "Watchlist scan: {} ({threshold}), sorted by {}",
            self.screen.mode.label(),
            self.screen.sort.label()
        );
        self.open_panel(PaneKind::Watchlist);
    }

    pub(super) fn technical_alert_command(&mut self, raw: &str) {
        if raw.eq_ignore_ascii_case("CLEAR") {
            let n = self.technical_alerts.len();
            self.technical_alerts.clear();
            self.status = format!("Cleared {n} local technical alerts");
        } else if raw.trim().is_empty() || raw.eq_ignore_ascii_case("LIST") {
            self.status = format!(
                "{} technical alerts. TECHALERT SYMBOL VOL 2 / RSI ABOVE 70 / SMA BELOW",
                self.technical_alerts.len()
            );
        } else if let Some((symbol, signal, target)) = parse_technical_alert_spec(raw) {
            let now = chrono::Utc::now().timestamp();
            let baseline =
                self.technicals.get(&symbol).filter(|s| s.ts > 0 && now - s.ts >= 0 && now - s.ts <= 72 * 3600);
            if let Some(alert) = TechnicalAlert::new(symbol.clone(), signal, target, baseline) {
                if !self
                    .technical_alerts
                    .iter()
                    .any(|a| !a.triggered && a.symbol == symbol && a.signal == signal && a.target == target)
                {
                    self.technical_alerts.push(alert);
                    self.technical_alerts.truncate(200);
                }
                self.last_technicals = None;
                self.status = format!("Watching {symbol}: {} {target} (daily-bar, one-shot)", signal.label());
            }
        } else {
            self.status =
                "TECHALERT SYMBOL VOL 2 | RSI ABOVE 70 | RSI BELOW 30 | SMA ABOVE/BELOW | LIST | CLEAR".into();
            return;
        }
        self.open_panel(PaneKind::Watchlist);
    }

    pub(super) fn on_technical_snapshots(&mut self, asked: &[String], snapshots: Vec<TechnicalSnapshot>) {
        let now = chrono::Utc::now().timestamp();
        for alert in &mut self.technical_alerts {
            if let Some(s) = snapshots.iter().find(|s| s.symbol == alert.symbol)
                && alert.observe(s, now)
            {
                let msg = format!(
                    "Technical alert: {} {} {} (daily candle {}, {})",
                    alert.symbol,
                    alert.signal.label(),
                    alert.target,
                    s.close,
                    if s.sample { "sample" } else { "observed" }
                );
                self.toasts.push((msg.clone(), web_time::Instant::now()));
                self.status = msg;
            }
        }
        self.technicals.retain(|key, _| !asked.contains(key) || snapshots.iter().any(|s| &s.symbol == key));
        for snapshot in snapshots {
            if self.technicals.get(&snapshot.symbol).is_some_and(|old| old.ts > snapshot.ts) {
                continue;
            }
            self.technicals.insert(snapshot.symbol.clone(), snapshot);
        }
    }

    pub(super) fn price_alert_command(&mut self, arg: &str) {
        if arg.eq_ignore_ascii_case("CLEAR") {
            let count = self.price_alerts.len();
            self.price_alerts.clear();
            self.status = format!("Cleared {count} local price alerts");
        } else if arg.trim().is_empty() || arg.eq_ignore_ascii_case("LIST") {
            self.status = format!("{} local alerts. Use PRICEALERT SYMBOL ABOVE|BELOW PRICE", self.price_alerts.len());
        } else if let Some((symbol, direction, target)) = parse_alert_spec(arg) {
            let baseline = self.quotes.get(&symbol).filter(|q| {
                let age = chrono::Utc::now().timestamp() - q.ts;
                q.ts > 0 && (0..=1800).contains(&age)
            });
            if let Some(alert) = PriceAlert::new(symbol.clone(), direction, target, baseline) {
                if !self
                    .price_alerts
                    .iter()
                    .any(|a| !a.triggered && a.symbol == symbol && a.direction == direction && a.target == target)
                {
                    self.price_alerts.push(alert);
                    self.price_alerts.truncate(200);
                }
                self.last_quotes = None;
                self.status =
                    format!("Monitoring {symbol}: {} {target} (one-shot, observed-price crossing)", direction.label());
            }
        } else {
            self.status = "PRICEALERT SYMBOL ABOVE|BELOW PRICE  ·  PRICEALERT LIST  ·  PRICEALERT CLEAR".into();
            return;
        }
        self.open_panel(PaneKind::Watchlist);
    }

    pub(super) fn on_market_quotes(&mut self, asked: &[String], quotes: Vec<Quote>) {
        let now = chrono::Utc::now().timestamp();
        for alert in &mut self.price_alerts {
            if let Some(q) = quotes.iter().find(|q| q.symbol == alert.symbol)
                && alert.observe(q, now)
            {
                let msg = format!(
                    "Price alert: {} crossed {} {} (observed {}, {})",
                    alert.symbol,
                    alert.direction.label(),
                    alert.target,
                    q.last,
                    q.source.as_deref().unwrap_or("unknown source"),
                );
                self.toasts.push((msg.clone(), web_time::Instant::now()));
                self.status = msg;
            }
        }
        merge_quotes(&mut self.quotes, asked, quotes);
    }

    pub(super) fn add_relative_price_alert(&mut self, symbol: String, direction: Direction) {
        let Some(q) = self.quotes.get(&symbol) else {
            self.status = format!("Cannot create price alert: no observed price for {symbol}");
            return;
        };
        let target = if direction == Direction::Above { q.last * 1.02 } else { q.last * 0.98 };
        if let Some(alert) = PriceAlert::new(symbol.clone(), direction, target, Some(q)) {
            if !self.price_alerts.iter().any(|a| {
                !a.triggered && a.symbol == alert.symbol && a.direction == alert.direction && a.target == alert.target
            }) {
                self.price_alerts.push(alert);
                self.price_alerts.truncate(200);
                self.last_quotes = None;
            }
            self.status = format!("Monitoring {symbol}: {} {target:.4}", direction.label());
        }
    }
}
