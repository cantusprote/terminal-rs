//! Read-only bridge to the *local* macOS Stocks app's SQLite market-data cache.
//!
//! This is not a StocksKit API client: it never uses private service methods,
//! sessions or credentials. Only already-cached symbols can be read. The cache
//! format is private and may change in future macOS releases.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Deserialize;
use tokio::sync::Mutex;
use tr_core::{Bar, Timeframe};

use crate::Quote;

// Apple CFAbsoluteTime counts seconds since 2001-01-01, not the Unix epoch.
const APPLE_EPOCH_OFFSET: f64 = 978_307_200.0;
const OPEN_MAX_AGE: f64 = 5.0 * 60.0;
// A cache record saying "closed" may become misleading after the next open.
// Do not reuse yesterday's close as though the cache had just refreshed.
const CLOSED_MAX_AGE: f64 = 30.0 * 60.0;
const SPARKLINE_MAX_AGE: f64 = 10.0 * 60.0;
const REFRESH_WAIT: Duration = Duration::from_secs(4);
const REFRESH_POLL: Duration = Duration::from_millis(200);
const OPEN_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_COOLDOWN: Duration = Duration::from_secs(120);
const OPEN_SPACING: Duration = Duration::from_millis(1_000);

#[derive(Debug, Default)]
struct RefreshGate {
    attempted: HashMap<String, Instant>,
    last_open: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshPlan {
    Launch,
    WaitForConcurrentLaunch,
    Skip,
}

#[derive(Debug, Clone)]
pub struct AppleStocksCache {
    cache_dir: PathBuf,
    refresh_gate: Arc<Mutex<RefreshGate>>,
}

#[derive(Deserialize)]
struct Envelope<T> {
    v: T,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredQuote {
    price: f64,
    price_change: f64,
    date_last_refreshed: Option<f64>,
    exchange_status: Option<String>,
}

#[derive(Deserialize)]
struct StoredSparkline {
    entries: Vec<StoredBar>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredBar {
    reference_date: f64,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: Option<f64>,
}

impl AppleStocksCache {
    /// Explicit opt-in; disabling leaves all prior Yahoo/Alpaca behavior intact.
    pub fn from_env() -> Option<Self> {
        let enabled = std::env::var("TR_APPLE_STOCKS_CACHE")
            .ok()
            .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"));
        if !enabled {
            return None;
        }
        let home = std::env::var_os("HOME")?;
        let dir = PathBuf::from(home).join("Library/Group Containers/group.com.apple.stocks/Library/Caches");
        let cache = Self::new(dir);
        if !cache.cache_dir.join("shared-database").is_file() {
            tracing::warn!("Apple Stocks cache unavailable; using standard market data");
            return None;
        }
        Some(cache)
    }

    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self { cache_dir: cache_dir.into(), refresh_gate: Arc::new(Mutex::new(RefreshGate::default())) }
    }

    /// Opens Apple's own symbol view via its registered deep link and then
    /// waits briefly for StocksKit to fill the ordinary local quote cache.
    /// This does not call any private API or access any session token.
    ///
    /// At most one symbol is opened per batch; an in-flight request for the
    /// same symbol is coalesced and unsuccessful symbols are rate-limited.
    /// Missing results are returned to the caller to route to Yahoo.
    pub async fn refresh_missing_quotes(&self, pairs: &[(String, String)]) -> Vec<Quote> {
        for (key, native) in pairs {
            let Some(url) = symbol_deep_link(native) else { continue };
            let plan = {
                let mut state = self.refresh_gate.lock().await;
                refresh_plan(&mut state, native, Instant::now())
            };
            if plan == RefreshPlan::Skip {
                continue;
            }
            if plan == RefreshPlan::Launch {
                let launched = tokio::task::spawn_blocking(move || {
                    Command::new("/usr/bin/open")
                        .arg("-g")
                        .arg(url)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .is_ok_and(|status| status.success())
                });
                let opened =
                    tokio::time::timeout(OPEN_TIMEOUT, launched).await.ok().and_then(Result::ok).unwrap_or(false);
                if !opened {
                    tracing::debug!(
                        "Apple Stocks could not open symbol {native}; will use standard market-data source"
                    );
                    continue;
                }
            }
            let started = Instant::now();
            let lookup = [(key.clone(), native.clone())];
            while started.elapsed() < REFRESH_WAIT {
                if let Ok(q) = self.quotes(&lookup)
                    && !q.is_empty()
                {
                    tracing::debug!("Apple Stocks cache refreshed for {native}");
                    return q;
                }
                tokio::time::sleep(REFRESH_POLL).await;
            }
            tracing::debug!("Apple Stocks cache did not refresh for {native}; falling back to standard source");
            // Avoid opening other stock symbols in this same batch; the Stocks
            // app navigates to a new symbol when a second deep link arrives.
            break;
        }
        Vec::new()
    }

    fn open(&self, file: &str) -> Result<Connection> {
        let path = self.cache_dir.join(file);
        let conn =
            Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
                .with_context(|| format!("open Apple Stocks cache {} read-only", path.display()))?;
        conn.busy_timeout(Duration::from_millis(200))?;
        Ok(conn)
    }

    fn fresh(updated_at: f64, max_age: f64, now: f64) -> bool {
        updated_at.is_finite() && (now - updated_at) >= -120.0 && (now - updated_at) <= max_age
    }

    /// Keys are pairs of (terminal market key, Yahoo/Apple native symbol).
    /// Never accesses `sds_auth_tokens`, user watchlists, or any private data.
    pub fn quotes(&self, keys: &[(String, String)]) -> Result<Vec<Quote>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.open("shared-database")?;
        let mut stmt = conn.prepare("SELECT updatedAt, valueJson FROM quotes WHERE id = ?1")?;
        let now = Utc::now().timestamp() as f64;
        let mut result = Vec::new();
        for (key, symbol) in keys {
            let row: Option<(f64, Vec<u8>)> =
                stmt.query_row(params![symbol], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
            if let Some((updated_at, json)) = row
                && let Ok(envelope) = serde_json::from_slice::<Envelope<StoredQuote>>(&json)
                && let Some(q) = parse_quote(key, updated_at, envelope.v, now)
            {
                result.push(q);
            }
        }
        Ok(result)
    }

    /// The Stocks sparkline normally has 10-minute OHLCV samples, not minute or
    /// full historical bars. Only return timeframes divisible by 10 minutes.
    pub fn history(&self, symbol: &str, tf: Timeframe) -> Result<Vec<Bar>> {
        if !matches!(tf, Timeframe::M30 | Timeframe::H1 | Timeframe::H4) {
            return Ok(Vec::new());
        }
        let conn = self.open("sparkline-database")?;
        let row: Option<(f64, Vec<u8>)> = conn
            .query_row("SELECT updatedAt, valueJson FROM sparklines WHERE id = ?1", params![symbol], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let Some((updated_at, json)) = row else { return Ok(Vec::new()) };
        if !Self::fresh(updated_at, SPARKLINE_MAX_AGE, Utc::now().timestamp() as f64) {
            return Ok(Vec::new());
        }
        let payload = serde_json::from_slice::<Envelope<StoredSparkline>>(&json)
            .context("parse Apple Stocks cached sparkline")?;
        Ok(parse_sparkline(payload.v.entries, tf))
    }
}

fn symbol_deep_link(symbol: &str) -> Option<String> {
    // Only market symbols, never arbitrary URL components or app arguments.
    if symbol.is_empty()
        || symbol.len() > 32
        || !symbol.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'^' | b'='))
    {
        return None;
    }
    let mut url = reqwest::Url::parse("stocks://").ok()?;
    url.query_pairs_mut().append_pair("symbol", symbol);
    Some(url.into())
}

fn refresh_plan(state: &mut RefreshGate, symbol: &str, now: Instant) -> RefreshPlan {
    if let Some(previous) = state.attempted.get(symbol) {
        if now.duration_since(*previous) < REFRESH_WAIT {
            return RefreshPlan::WaitForConcurrentLaunch;
        }
        if now.duration_since(*previous) < RETRY_COOLDOWN {
            return RefreshPlan::Skip;
        }
    }
    if state.last_open.is_some_and(|prev| now.duration_since(prev) < OPEN_SPACING) {
        return RefreshPlan::Skip;
    }
    state.attempted.retain(|_, instant| now.duration_since(*instant) < RETRY_COOLDOWN);
    state.attempted.insert(symbol.to_owned(), now);
    state.last_open = Some(now);
    RefreshPlan::Launch
}

fn parse_quote(symbol: &str, updated_at: f64, s: StoredQuote, now: f64) -> Option<Quote> {
    let max_age = if s.exchange_status.as_deref() == Some("open") { OPEN_MAX_AGE } else { CLOSED_MAX_AGE };
    if !AppleStocksCache::fresh(updated_at, max_age, now) || !s.price.is_finite() || s.price <= 0.0 {
        return None;
    }
    let prev_close = s.price - s.price_change;
    if !prev_close.is_finite() || prev_close <= 0.0 {
        return None;
    }
    let stamp = s.date_last_refreshed.map(|t| t + APPLE_EPOCH_OFFSET).unwrap_or(updated_at);
    if !stamp.is_finite() || stamp < 0.0 || stamp > now + 120.0 {
        return None;
    }
    Some(Quote { symbol: symbol.to_string(), last: s.price, prev_close, ts: stamp as i64, spark: vec![] })
}

fn parse_sparkline(mut entries: Vec<StoredBar>, tf: Timeframe) -> Vec<Bar> {
    // Reject non-10-minute data rather than silently passing it off as 30m bars.
    entries.sort_by(|a, b| a.reference_date.total_cmp(&b.reference_date));
    if entries.len() < 2 {
        return Vec::new();
    }
    let mut intervals: Vec<i64> = entries
        .windows(2)
        .map(|pair| (pair[1].reference_date - pair[0].reference_date).round() as i64)
        .filter(|d| *d > 0 && *d < 1_800)
        .collect();
    intervals.sort_unstable();
    // The final bar can be partial. Require the typical interval to be 10m.
    if intervals.get(intervals.len() / 2).is_none_or(|d| !(540..=660).contains(d)) {
        return Vec::new();
    }
    let bars: Vec<Bar> = entries
        .into_iter()
        .filter(|b| {
            b.reference_date.is_finite()
                && b.open.is_finite()
                && b.high.is_finite()
                && b.low.is_finite()
                && b.close.is_finite()
        })
        .filter(|b| b.open > 0.0 && b.close > 0.0 && b.high >= b.open.max(b.close) && b.low <= b.open.min(b.close))
        .map(|b| {
            Bar::new(
                (b.reference_date + APPLE_EPOCH_OFFSET) as i64,
                b.open,
                b.high,
                b.low,
                b.close,
                b.volume.unwrap_or(0.0).max(0.0),
            )
        })
        .collect();
    tf.resample(&bars)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn apple_epoch_and_change_are_decoded() {
        let s: Envelope<StoredQuote> = serde_json::from_str(
            r#"{"v":{"price":349.51,"priceChange":17.61,"dateLastRefreshed":815538119,"exchangeStatus":"open"}}"#,
        )
        .unwrap();
        let q = parse_quote("AAPL US Equity", 1_793_845_319.0, s.v, 1_793_845_400.0).unwrap();
        assert!((q.prev_close - 331.9).abs() < 0.001);
        assert_eq!(q.ts, 1_793_845_319);
        assert!(q.spark.is_empty());
    }

    #[test]
    fn stale_open_quotes_are_rejected() {
        let q: Envelope<StoredQuote> = serde_json::from_str(
            r#"{"v":{"price":100.0,"priceChange":1.0,"dateLastRefreshed":815538119,"exchangeStatus":"open"}}"#,
        )
        .unwrap();
        assert!(parse_quote("AAPL", 1_793_845_319.0, q.v, 1_793_846_000.0).is_none());
    }

    #[test]
    fn only_compatible_10m_bars_are_aggregated() {
        let entries = (0..6)
            .map(|i| StoredBar {
                reference_date: 815_495_400.0 + (i * 600) as f64,
                open: 100.0 + i as f64,
                high: 102.0 + i as f64,
                low: 99.0 + i as f64,
                close: 101.0 + i as f64,
                volume: Some(10.0),
            })
            .collect();
        let bars = parse_sparkline(entries, Timeframe::M30);
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].volume, 30.0);
        assert_eq!(bars[1].close, 106.0);
    }

    #[test]
    fn reads_only_the_two_quote_and_sparkline_tables() {
        let root = std::env::temp_dir().join(format!(
            "terminal-apple-stocks-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let now = Utc::now().timestamp() as f64;
        let c = Connection::open(root.join("shared-database")).unwrap();
        c.execute_batch(
            "CREATE TABLE quotes (id TEXT PRIMARY KEY, updatedAt DOUBLE NOT NULL, valueJson BLOB NOT NULL);",
        )
        .unwrap();
        let quote = serde_json::json!({"v": {
            "price": 123.45,
            "priceChange": 3.45,
            "dateLastRefreshed": now - APPLE_EPOCH_OFFSET,
            "exchangeStatus": "open"
        }});
        c.execute("INSERT INTO quotes VALUES (?1, ?2, ?3)", params!["AAPL", now, quote.to_string().into_bytes()])
            .unwrap();
        drop(c);

        let c = Connection::open(root.join("sparkline-database")).unwrap();
        c.execute_batch(
            "CREATE TABLE sparklines (id TEXT PRIMARY KEY, updatedAt DOUBLE NOT NULL, valueJson BLOB NOT NULL);",
        )
        .unwrap();
        let anchor = ((now as i64 / 1800) * 1800) as f64;
        let entries: Vec<_> = (0..6)
            .map(|i| {
                serde_json::json!({
                    "referenceDate": anchor + (i as f64) * 600.0 - 3600.0 - APPLE_EPOCH_OFFSET,
                    "open": 100.0, "high": 102.0, "low": 99.0, "close": 101.0, "volume": 12
                })
            })
            .collect();
        let sparkline = serde_json::json!({"v":{"entries":entries}});
        c.execute(
            "INSERT INTO sparklines VALUES (?1, ?2, ?3)",
            params!["AAPL", now, sparkline.to_string().into_bytes()],
        )
        .unwrap();
        drop(c);

        let cache = AppleStocksCache::new(&root);
        let quotes =
            cache.quotes(&[("AAPL US Equity".into(), "AAPL".into()), ("MISSING".into(), "MISSING".into())]).unwrap();
        assert_eq!(quotes.len(), 1);
        assert_eq!(quotes[0].last, 123.45);
        assert!((quotes[0].prev_close - 120.0).abs() < 0.001);
        let recent = cache.history("AAPL", Timeframe::M30).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].volume, 36.0);
        assert!(cache.history("AAPL", Timeframe::M15).unwrap().is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn stocks_deep_link_encodes_symbols_and_rejects_untrusted_arguments() {
        assert_eq!(symbol_deep_link("ORCL").as_deref(), Some("stocks://?symbol=ORCL"));
        assert_eq!(symbol_deep_link("005930.KS").as_deref(), Some("stocks://?symbol=005930.KS"));
        assert_eq!(symbol_deep_link("^KS11").as_deref(), Some("stocks://?symbol=%5EKS11"));
        assert!(symbol_deep_link("AAPL&foo=bar").is_none());
        assert!(symbol_deep_link("--args").is_none_or(|s| !s.contains('&')));
        assert!(symbol_deep_link("AAPL\n--args").is_none());
        assert!(symbol_deep_link("").is_none());
    }

    #[test]
    fn duplicate_requests_are_coalesced_and_retry_is_throttled() {
        let mut state = RefreshGate::default();
        let now = Instant::now();
        assert_eq!(refresh_plan(&mut state, "ORCL", now), RefreshPlan::Launch);
        assert_eq!(
            refresh_plan(&mut state, "ORCL", now + Duration::from_millis(100)),
            RefreshPlan::WaitForConcurrentLaunch
        );
        assert_eq!(refresh_plan(&mut state, "ADBE", now + Duration::from_millis(100)), RefreshPlan::Skip);
        assert_eq!(refresh_plan(&mut state, "ORCL", now + Duration::from_secs(5)), RefreshPlan::Skip);
        assert_eq!(refresh_plan(&mut state, "ADBE", now + Duration::from_secs(5)), RefreshPlan::Launch);
        assert_eq!(
            refresh_plan(&mut state, "ORCL", now + RETRY_COOLDOWN + Duration::from_secs(10)),
            RefreshPlan::Launch
        );
    }
}
