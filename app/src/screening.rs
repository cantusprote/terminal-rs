//! Local watchlist screening and one-shot price-crossing alerts.
//!
//! Only already-fetched market observations are used; no simulated or
//! invented prices and no trading/transaction capability.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tr_core::Bar;
use tr_data::Quote;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ScreenMode {
    #[default]
    All,
    Gainers,
    Losers,
    Movers,
    VolumeSurge,
    AboveSma20,
    BelowSma20,
    RsiHigh,
    RsiLow,
}

impl ScreenMode {
    pub const ALL: [Self; 9] = [
        Self::All,
        Self::Gainers,
        Self::Losers,
        Self::Movers,
        Self::VolumeSurge,
        Self::AboveSma20,
        Self::BelowSma20,
        Self::RsiHigh,
        Self::RsiLow,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Gainers => "Gainers",
            Self::Losers => "Losers",
            Self::Movers => "Moves ±",
            Self::VolumeSurge => "Volume surge",
            Self::AboveSma20 => "Above SMA20",
            Self::BelowSma20 => "Below SMA20",
            Self::RsiHigh => "RSI ≥ 70",
            Self::RsiLow => "RSI ≤ 30",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ScreenSort {
    #[default]
    Watchlist,
    Gainers,
    Losers,
    Symbol,
}

impl ScreenSort {
    pub const ALL: [Self; 4] = [Self::Watchlist, Self::Gainers, Self::Losers, Self::Symbol];
    pub fn label(self) -> &'static str {
        match self {
            Self::Watchlist => "Watchlist order",
            Self::Gainers => "Change % ↓",
            Self::Losers => "Change % ↑",
            Self::Symbol => "Symbol A–Z",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenSettings {
    pub mode: ScreenMode,
    pub sort: ScreenSort,
    /// Positive minimum absolute percentage move for the change filters.
    pub min_move_pct: f64,
    /// Minimum latest daily volume / previous 20 daily volumes average.
    pub min_volume_ratio: f64,
    /// None allows all observations; Some(n) requires a quote <= n minutes old.
    pub max_age_minutes: Option<u32>,
}

impl Default for ScreenSettings {
    fn default() -> Self {
        Self {
            mode: ScreenMode::All,
            sort: ScreenSort::Watchlist,
            min_move_pct: 1.0,
            min_volume_ratio: 1.5,
            max_age_minutes: None,
        }
    }
}

/// Daily-bar technicals. Values are computed from the observation series,
/// not from any order/portfolio state. The current session's volume may be
/// incomplete, so ratios are a progress indicator, not a guaranteed daily close.
#[derive(Debug, Clone, PartialEq)]
pub struct TechnicalSnapshot {
    pub symbol: String,
    pub ts: i64,
    pub close: f64,
    pub volume: f64,
    pub sma20: Option<f64>,
    pub rsi14: Option<f64>,
    pub volume_ratio20: Option<f64>,
    pub sample: bool,
}

impl TechnicalSnapshot {
    pub fn from_bars(symbol: String, bars: &[Bar], sample: bool) -> Option<Self> {
        let last = *bars.last()?;
        if bars.windows(2).any(|w| w[0].ts >= w[1].ts)
            || !bars.iter().all(|b| {
                [b.open, b.high, b.low, b.close, b.volume].iter().all(|v| v.is_finite())
                    && b.volume >= 0.0
                    && b.high >= b.open.max(b.close)
                    && b.low <= b.open.min(b.close)
            })
        {
            return None;
        }
        let sma20 = (bars.len() >= 20).then(|| bars.iter().rev().take(20).map(|b| b.close).sum::<f64>() / 20.0);
        let volume_ratio20 = if bars.len() > 20 {
            let avg = bars.iter().rev().skip(1).take(20).map(|b| b.volume).sum::<f64>() / 20.0;
            (avg > 0.0).then_some(last.volume / avg)
        } else {
            None
        };
        let rsi14 = if bars.len() >= 15 {
            let mut gains = 0.0;
            let mut losses = 0.0;
            for pair in bars[..15].windows(2) {
                let diff = pair[1].close - pair[0].close;
                gains += diff.max(0.0);
                losses += (-diff).max(0.0);
            }
            let mut avg_gain = gains / 14.0;
            let mut avg_loss = losses / 14.0;
            for pair in bars[14..].windows(2) {
                let diff = pair[1].close - pair[0].close;
                avg_gain = (avg_gain * 13.0 + diff.max(0.0)) / 14.0;
                avg_loss = (avg_loss * 13.0 + (-diff).max(0.0)) / 14.0;
            }
            Some(if avg_gain == 0.0 && avg_loss == 0.0 {
                50.0
            } else if avg_loss == 0.0 {
                100.0
            } else {
                100.0 - 100.0 / (1.0 + avg_gain / avg_loss)
            })
        } else {
            None
        };
        Some(Self { symbol, ts: last.ts, close: last.close, volume: last.volume, sma20, rsi14, volume_ratio20, sample })
    }

    fn fingerprint(&self) -> (i64, u64, u64) {
        (self.ts, self.close.to_bits(), self.volume.to_bits())
    }
}

pub fn screened_symbols(
    watchlist: &[String],
    quotes: &HashMap<String, Quote>,
    technicals: &HashMap<String, TechnicalSnapshot>,
    settings: &ScreenSettings,
    now: i64,
) -> Vec<String> {
    let threshold = settings.min_move_pct.clamp(0.0, 1000.0);
    let mut rows: Vec<String> = watchlist
        .iter()
        .filter(|s| {
            let q = quotes.get(s.as_str());
            let meets_age = settings.max_age_minutes.is_none_or(|mins| {
                q.is_some_and(|q| q.ts > 0 && now >= q.ts - 120 && now - q.ts <= i64::from(mins) * 60)
            });
            if !meets_age {
                return false;
            }
            match settings.mode {
                ScreenMode::All => true,
                ScreenMode::VolumeSurge => technicals
                    .get(s.as_str())
                    .and_then(|t| t.volume_ratio20)
                    .is_some_and(|v| v.is_finite() && v >= settings.min_volume_ratio.max(0.0)),
                ScreenMode::AboveSma20 => {
                    technicals.get(s.as_str()).and_then(|t| t.sma20.map(|sma| t.close > sma)) == Some(true)
                }
                ScreenMode::BelowSma20 => {
                    technicals.get(s.as_str()).and_then(|t| t.sma20.map(|sma| t.close < sma)) == Some(true)
                }
                ScreenMode::RsiHigh => technicals.get(s.as_str()).and_then(|t| t.rsi14).is_some_and(|r| r >= 70.0),
                ScreenMode::RsiLow => technicals.get(s.as_str()).and_then(|t| t.rsi14).is_some_and(|r| r <= 30.0),
                _ => {
                    let Some(q) = q.filter(|q| valid(q)) else { return false };
                    let pct = q.change_pct();
                    match settings.mode {
                        ScreenMode::Gainers => pct >= threshold,
                        ScreenMode::Losers => pct <= -threshold,
                        ScreenMode::Movers => pct.abs() >= threshold,
                        _ => unreachable!("other scan types handled above"),
                    }
                }
            }
        })
        .cloned()
        .collect();
    match settings.sort {
        ScreenSort::Watchlist => {}
        ScreenSort::Symbol => rows.sort(),
        ScreenSort::Gainers | ScreenSort::Losers => {
            let pct = |s: &String| quotes.get(s).filter(|q| valid(q)).map(Quote::change_pct);
            rows.sort_by(|a, b| match (pct(a), pct(b)) {
                (Some(x), Some(y)) => {
                    let cmp = x.total_cmp(&y);
                    if settings.sort == ScreenSort::Gainers { cmp.reverse() } else { cmp }.then_with(|| a.cmp(b))
                }
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.cmp(b),
            });
        }
    }
    rows
}

fn valid(q: &Quote) -> bool {
    q.last.is_finite() && q.last > 0.0 && q.prev_close.is_finite() && q.prev_close > 0.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Above,
    Below,
}

impl Direction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Above => "crosses above",
            Self::Below => "crosses below",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceAlert {
    pub symbol: String,
    pub direction: Direction,
    pub target: f64,
    #[serde(default)]
    pub last_price: Option<f64>,
    #[serde(default)]
    pub last_ts: i64,
    #[serde(default)]
    pub triggered: bool,
}

/// `PRICEALERT AAPL ABOVE 210` / `PRICEALERT 005930.KS BELOW 72000`.
/// Reuse the terminal's market-key resolver rather than guessing assets.
pub fn parse_alert_spec(raw: &str) -> Option<(String, Direction, f64)> {
    let parts: Vec<&str> = raw.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }
    let target: f64 = parts.last()?.replace(',', "").parse().ok()?;
    if !target.is_finite() || target <= 0.0 {
        return None;
    }
    let direction = match parts[parts.len() - 2].to_ascii_uppercase().as_str() {
        "ABOVE" | "UP" => Direction::Above,
        "BELOW" | "DOWN" => Direction::Below,
        _ => return None,
    };
    let sym = &parts[..parts.len() - 2];
    let (sec, used) = tr_core::Security::resolve_tokens(sym)?;
    (used == sym.len()).then(|| (sec.key(), direction, target))
}

impl PriceAlert {
    pub fn new(symbol: String, direction: Direction, target: f64, existing: Option<&Quote>) -> Option<Self> {
        if symbol.is_empty() || !target.is_finite() || target <= 0.0 {
            return None;
        }
        let initial = existing.filter(|q| valid(q) && q.ts > 0 && q.source.as_deref().is_some_and(|s| s != "Sample"));
        Some(Self {
            symbol,
            direction,
            target,
            last_price: initial.map(|q| q.last),
            last_ts: initial.map_or(0, |q| q.ts),
            triggered: false,
        })
    }

    /// Returns true only on a crossing by a *newer, recent, real* observation.
    /// A first quote establishes the reference price and cannot trigger.
    pub fn observe(&mut self, q: &Quote, now: i64) -> bool {
        if self.triggered
            || q.symbol != self.symbol
            || !valid(q)
            || !q.source.as_deref().is_some_and(|s| s != "Sample")
            || q.ts <= self.last_ts
            || q.ts <= 0
            || now - q.ts > 1800
            || q.ts > now + 120
        {
            return false;
        }
        let crossed = self.last_price.is_some_and(|previous| match self.direction {
            Direction::Above => previous < self.target && q.last >= self.target,
            Direction::Below => previous > self.target && q.last <= self.target,
        });
        self.last_price = Some(q.last);
        self.last_ts = q.ts;
        self.triggered = crossed;
        crossed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TechnicalSignal {
    VolumeAbove,
    RsiAbove,
    RsiBelow,
    SmaAbove,
    SmaBelow,
}

impl TechnicalSignal {
    pub fn label(self) -> &'static str {
        match self {
            Self::VolumeAbove => "volume ratio exceeds",
            Self::RsiAbove => "RSI14 crosses above",
            Self::RsiBelow => "RSI14 crosses below",
            Self::SmaAbove => "price crosses above SMA20",
            Self::SmaBelow => "price crosses below SMA20",
        }
    }
    fn comparable(self, s: &TechnicalSnapshot) -> Option<f64> {
        match self {
            Self::VolumeAbove => s.volume_ratio20,
            Self::RsiAbove => s.rsi14,
            Self::RsiBelow => s.rsi14.map(|r| -r),
            Self::SmaAbove => s.sma20.map(|avg| s.close - avg),
            Self::SmaBelow => s.sma20.map(|avg| avg - s.close),
        }
    }
    fn comparison_target(self, target: f64) -> f64 {
        if self == Self::RsiBelow { -target } else { target }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TechnicalAlert {
    pub symbol: String,
    pub signal: TechnicalSignal,
    pub target: f64,
    #[serde(default)]
    pub previous: Option<f64>,
    #[serde(default)]
    pub last_marker: Option<(i64, u64, u64)>,
    #[serde(default)]
    pub triggered: bool,
}

impl TechnicalAlert {
    pub fn new(
        symbol: String,
        signal: TechnicalSignal,
        target: f64,
        initial: Option<&TechnicalSnapshot>,
    ) -> Option<Self> {
        if symbol.is_empty() || !target.is_finite() || target < 0.0 {
            return None;
        }
        let initial = initial.filter(|s| !s.sample && s.symbol == symbol);
        Some(Self {
            symbol,
            signal,
            target,
            previous: initial.and_then(|s| signal.comparable(s)),
            last_marker: initial.map(TechnicalSnapshot::fingerprint),
            triggered: false,
        })
    }

    /// A new daily candle or a revised current daily bar can cross a
    /// threshold, but re-fetching identical OHLCV cannot alert repeatedly.
    pub fn observe(&mut self, snapshot: &TechnicalSnapshot, now: i64) -> bool {
        if self.triggered
            || snapshot.sample
            || snapshot.symbol != self.symbol
            || snapshot.ts <= 0
            || snapshot.ts > now + 120
            || now - snapshot.ts > 72 * 3600
            || self.last_marker.is_some_and(|(previous_ts, _, _)| snapshot.ts < previous_ts)
            || self.last_marker == Some(snapshot.fingerprint())
        {
            return false;
        }
        let Some(current) = self.signal.comparable(snapshot).filter(|v| v.is_finite()) else {
            return false;
        };
        let threshold = self.signal.comparison_target(self.target);
        let crossed = self.previous.is_some_and(|prev| prev < threshold && current >= threshold);
        self.previous = Some(current);
        self.last_marker = Some(snapshot.fingerprint());
        self.triggered = crossed;
        crossed
    }
}

/// `TECHALERT AAPL VOL 2`, `TECHALERT AAPL RSI ABOVE 70`,
/// `TECHALERT AAPL SMA BELOW`.
pub fn parse_technical_alert_spec(input: &str) -> Option<(String, TechnicalSignal, f64)> {
    let w: Vec<&str> = input.split_whitespace().collect();
    if w.len() < 3 {
        return None;
    }
    let end = w.len();
    let upper = |i: usize| w[i].to_ascii_uppercase();
    let (prefix, signal, target) = if end >= 4 && matches!(upper(end - 3).as_str(), "RSI" | "RSI14") {
        let value: f64 = w[end - 1].parse().ok()?;
        let signal = match upper(end - 2).as_str() {
            "ABOVE" | "UP" => TechnicalSignal::RsiAbove,
            "BELOW" | "DOWN" => TechnicalSignal::RsiBelow,
            _ => return None,
        };
        if !(0.0..=100.0).contains(&value) {
            return None;
        }
        (end - 3, signal, value)
    } else if end >= 4 && matches!(upper(end - 3).as_str(), "VOL" | "VOLUME") && upper(end - 2) == "ABOVE" {
        let value: f64 = w[end - 1].parse().ok()?;
        (end - 3, TechnicalSignal::VolumeAbove, value)
    } else if matches!(upper(end - 2).as_str(), "VOL" | "VOLUME") {
        let value: f64 = w[end - 1].parse().ok()?;
        (end - 2, TechnicalSignal::VolumeAbove, value)
    } else if end >= 3 && matches!(upper(end - 2).as_str(), "SMA" | "SMA20") {
        let signal = match upper(end - 1).as_str() {
            "ABOVE" | "UP" => TechnicalSignal::SmaAbove,
            "BELOW" | "DOWN" => TechnicalSignal::SmaBelow,
            _ => return None,
        };
        (end - 2, signal, 0.0)
    } else {
        return None;
    };
    if !target.is_finite() || target < 0.0 || (signal == TechnicalSignal::VolumeAbove && target == 0.0) {
        return None;
    }
    let (security, used) = tr_core::Security::resolve_tokens(&w[..prefix])?;
    (used == prefix).then(|| (security.key(), signal, target))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn quote(key: &str, last: f64, now: i64) -> Quote {
        Quote {
            symbol: key.into(),
            last,
            prev_close: 100.0,
            ts: now,
            spark: vec![],
            source: Some("Yahoo Finance".into()),
        }
    }

    #[test]
    fn watchlist_screener_filters_and_orders_without_inventing_missing_data() {
        let now = 10000;
        let symbols = ["B", "MISSING", "A", "C"].map(String::from).to_vec();
        let quotes = HashMap::from([
            ("A".into(), quote("A", 106.0, now)),
            ("B".into(), quote("B", 98.0, now)),
            ("C".into(), quote("C", 101.0, now - 3600)),
        ]);
        let mut s = ScreenSettings { mode: ScreenMode::Movers, min_move_pct: 2.0, ..Default::default() };
        assert_eq!(screened_symbols(&symbols, &quotes, &HashMap::new(), &s, now), ["B", "A"]);
        s.mode = ScreenMode::All;
        s.sort = ScreenSort::Gainers;
        assert_eq!(screened_symbols(&symbols, &quotes, &HashMap::new(), &s, now), ["A", "C", "B", "MISSING"]);
        s.sort = ScreenSort::Watchlist;
        s.max_age_minutes = Some(5);
        assert_eq!(screened_symbols(&symbols, &quotes, &HashMap::new(), &s, now), ["B", "A"]);
    }

    #[test]
    fn price_alert_requires_a_real_recent_new_crossing_and_fires_once() {
        let mut a = PriceAlert::new("A".into(), Direction::Above, 105.0, None).unwrap();
        assert!(!a.observe(&quote("A", 104.0, 1000), 1000));
        assert!(!a.observe(&quote("A", 110.0, 1000), 1010)); // old same-time replay
        assert!(!a.observe(&quote("A", 110.0, 1010), 3000)); // stale data
        assert!(a.observe(&quote("A", 106.0, 1020), 1020));
        assert!(!a.observe(&quote("A", 108.0, 1030), 1030));
        let mut b = PriceAlert::new("A".into(), Direction::Below, 95.0, Some(&quote("A", 100.0, 1200))).unwrap();
        assert!(b.observe(&quote("A", 94.0, 1210), 1210));
    }

    #[test]
    fn sample_observations_cannot_trigger_price_alerts() {
        let mut a = PriceAlert::new("A".into(), Direction::Above, 105.0, None).unwrap();
        let mut sample = quote("A", 108.0, 1200);
        sample.source = Some("Sample".into());
        assert!(!a.observe(&sample, 1200));
        assert!(a.last_price.is_none());
    }

    #[test]
    fn older_saved_alerts_deserialize_with_new_defaults() {
        let json = r#"{"symbol":"A","direction":"Above","target":105}"#;
        let a: PriceAlert = serde_json::from_str(json).unwrap();
        assert!(!a.triggered && a.last_ts == 0 && a.last_price.is_none());
    }

    #[test]
    fn daily_technical_snapshot_computes_volume_sma20_and_rsi14() {
        let bars: Vec<Bar> = (0..25)
            .map(|i| {
                let c = 100.0 + f64::from(i);
                Bar::new(
                    1_790_000_000 + i64::from(i) * 86_400,
                    c,
                    c + 1.0,
                    c - 1.0,
                    c,
                    if i == 24 { 350.0 } else { 100.0 },
                )
            })
            .collect();
        let x = TechnicalSnapshot::from_bars("A".into(), &bars, false).unwrap();
        assert_eq!(x.volume_ratio20, Some(3.5));
        assert!((x.sma20.unwrap() - 114.5).abs() < 1e-9);
        assert_eq!(x.rsi14, Some(100.0));
        assert!(x.close > x.sma20.unwrap());
        let filtered = screened_symbols(
            &["A".into()],
            &HashMap::new(),
            &HashMap::from([("A".into(), x.clone())]),
            &ScreenSettings { mode: ScreenMode::VolumeSurge, min_volume_ratio: 2.0, ..Default::default() },
            x.ts,
        );
        assert_eq!(filtered, ["A"]);
    }

    #[test]
    fn technical_alert_requires_real_new_cross_and_never_orders() {
        let mut first = TechnicalSnapshot {
            symbol: "A".into(),
            ts: 1_790_000_000,
            close: 100.0,
            volume: 100.0,
            sma20: Some(101.0),
            rsi14: Some(65.0),
            volume_ratio20: Some(1.0),
            sample: false,
        };
        let mut a = TechnicalAlert::new("A".into(), TechnicalSignal::RsiAbove, 70.0, Some(&first)).unwrap();
        assert!(!a.observe(&first, first.ts));
        first.rsi14 = Some(75.0);
        // An identical OHLCV observation is not an independent market update.
        assert!(!a.observe(&first, first.ts));
        first.ts += 86_400;
        assert!(a.observe(&first, first.ts));
        assert!(!a.observe(&first, first.ts));
        let mut volume = TechnicalAlert::new("A".into(), TechnicalSignal::VolumeAbove, 2.0, None).unwrap();
        first.volume_ratio20 = Some(1.5);
        assert!(!volume.observe(&first, first.ts));
        first.volume = 350.0;
        first.volume_ratio20 = Some(3.5);
        assert!(volume.observe(&first, first.ts));
        let mut fake = TechnicalAlert::new("A".into(), TechnicalSignal::SmaAbove, 0.0, None).unwrap();
        first.sample = true;
        assert!(!fake.observe(&first, first.ts));
    }

    #[test]
    fn technical_alert_command_parsing_supports_market_keys() {
        assert_eq!(
            parse_technical_alert_spec("AAPL VOL 2"),
            Some(("AAPL US Equity".into(), TechnicalSignal::VolumeAbove, 2.0))
        );
        assert_eq!(
            parse_technical_alert_spec("005930.KS RSI ABOVE 70"),
            Some(("005930.KS US Equity".into(), TechnicalSignal::RsiAbove, 70.0))
        );
        assert_eq!(
            parse_technical_alert_spec("BTCUSD CRNCY SMA BELOW"),
            Some(("BTCUSD Curncy".into(), TechnicalSignal::SmaBelow, 0.0))
        );
        for bad in ["AAPL RSI ABOVE NaN", "AAPL VOLUME 0", "AAPL SMA SIDEWAYS", "AAPL UNKNOWN 2"] {
            assert!(parse_technical_alert_spec(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn parses_price_alerts_without_accepting_invalid_prices_or_assets() {
        assert_eq!(parse_alert_spec("AAPL ABOVE 210"), Some(("AAPL US Equity".into(), Direction::Above, 210.0)));
        assert_eq!(
            parse_alert_spec("005930.KS BELOW 72,000"),
            Some(("005930.KS US Equity".into(), Direction::Below, 72000.0))
        );
        assert!(parse_alert_spec("AAPL UP NaN").is_none());
        assert!(parse_alert_spec("AAPL BELOW -1").is_none());
        assert!(parse_alert_spec("AAPL SOMEWORD ABOVE 100").is_none());
    }
}
