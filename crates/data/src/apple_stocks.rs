//! Read-only bridge to the *local* macOS Stocks app's SQLite market-data cache.
//!
//! This is not a StocksKit API client: it never uses private service methods,
//! sessions or credentials. The existing local cache is read without opening
//! Apple's Stocks application. Opening new symbols via `stocks://` is a
//! separate, explicit opt-in (TR_APPLE_STOCKS_LAUNCH=1), never the default.
//! The cache format is private and may change in future macOS releases.

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
/// Opening dozens of symbols in Stocks would disrupt the user's own Stocks
/// browsing. Other cache misses fall back to Yahoo immediately.
const MAX_REFRESH_PER_BATCH: usize = 3;

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
    /// Reading Stocks' cache must not silently launch the Stocks application.
    launch_enabled: bool,
    refresh_gate: Arc<Mutex<RefreshGate>>,
    /// Stocks uses one selected-symbol view. Concurrent deep links would
    /// interrupt each other, so only one quote-loading batch may navigate.
    navigation: Arc<Mutex<()>>,
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
    price_source: Option<String>,
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
        let mut cache = Self::new(dir);
        cache.launch_enabled = std::env::var("TR_APPLE_STOCKS_LAUNCH")
            .ok()
            .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"));
        if !cache.cache_dir.join("shared-database").is_file() {
            tracing::warn!("Apple Stocks cache unavailable; using standard market data");
            return None;
        }
        Some(cache)
    }

    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            cache_dir: cache_dir.into(),
            launch_enabled: false,
            refresh_gate: Arc::new(Mutex::new(RefreshGate::default())),
            navigation: Arc::new(Mutex::new(())),
        }
    }

    /// Only when TR_APPLE_STOCKS_LAUNCH=1 was explicitly set, opens Apple's
    /// own symbol view and waits for StocksKit's ordinary local quote cache.
    /// Otherwise the caller immediately falls back to Yahoo without opening
    /// or relaunching Stocks, even when the cache is stale.
    /// This does not call any private API or access any session token.
    ///
    /// At most three missing symbols are opened per batch; an in-flight
    /// request for the same symbol is coalesced and attempts are rate-limited.
    /// Missing results are returned to the caller to route to Yahoo.
    pub async fn refresh_missing_quotes(&self, pairs: &[(String, String)]) -> Vec<Quote> {
        // App users frequently quit Stocks with Cmd+Q. A cache *read* opt-in
        // must never override that decision by repeatedly relaunching Stocks.
        if !self.launch_enabled {
            return Vec::new();
        }
        // Keep the normal provider fallback responsive while another history
        // or watchlist request navigates the Stocks application.
        let Ok(_navigation) = self.navigation.try_lock() else {
            return Vec::new();
        };
        let mut refreshed = Vec::new();
        let mut attempted = 0;
        for (key, native) in pairs {
            if attempted >= MAX_REFRESH_PER_BATCH {
                break;
            }
            let Some(url) = symbol_deep_link(native) else { continue };
            // Allow a serial batch to open each symbol without interrupting
            // Stocks immediately after it began loading the previous one.
            let delay = {
                let state = self.refresh_gate.lock().await;
                state.last_open.map(|t| OPEN_SPACING.saturating_sub(t.elapsed())).unwrap_or_default()
            };
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let plan = {
                let mut state = self.refresh_gate.lock().await;
                refresh_plan(&mut state, native, Instant::now())
            };
            if plan == RefreshPlan::Skip {
                continue;
            }
            attempted += 1;
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
                    refreshed.extend(q);
                    break;
                }
                tokio::time::sleep(REFRESH_POLL).await;
            }
            if !refreshed.iter().any(|q| q.symbol == *key) {
                tracing::debug!("Apple Stocks cache did not refresh for {native}; falling back to standard source");
            }
        }
        refreshed
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
    let delayed = s.price_source.as_ref().is_some_and(|source| source.to_ascii_lowercase().contains("delayed"));
    let max_age = if s.exchange_status.as_deref() == Some("open") && !delayed { OPEN_MAX_AGE } else { CLOSED_MAX_AGE };
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
    // Some SQLite rows were touched recently but still contain an older
    // exchange quote. Timestamp validity must use the actual observation.
    if !AppleStocksCache::fresh(stamp, max_age, now) {
        return None;
    }
    Some(Quote {
        symbol: symbol.to_string(),
        last: s.price,
        prev_close,
        ts: stamp as i64,
        spark: vec![],
        source: Some(if delayed { "Apple Stocks (delayed)" } else { "Apple Stocks" }.into()),
    })
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
    fn updated_sqlite_row_does_not_make_old_market_observation_fresh() {
        let now = 1_793_846_000.0;
        let old_market_stamp = now - 7_200.0 - APPLE_EPOCH_OFFSET;
        let q: Envelope<StoredQuote> = serde_json::from_value(serde_json::json!({
            "v": {"price": 230.0, "priceChange": 5.0, "dateLastRefreshed": old_market_stamp,
                  "exchangeStatus": "open", "priceSource": "nasdaqRealtime"}
        }))
        .unwrap();
        assert!(parse_quote("AAPL", now - 5.0, q.v, now).is_none());
    }

    #[test]
    fn delayed_exchange_quotes_are_labeled_and_bounded() {
        let now = 1_793_846_000.0;
        let q: Envelope<StoredQuote> = serde_json::from_value(serde_json::json!({
            "v": {"price": 15000.0, "priceChange": -200.0,
                  "dateLastRefreshed": now - 1_200.0 - APPLE_EPOCH_OFFSET,
                  "exchangeStatus": "open", "priceSource": "genericDelayed"}
        }))
        .unwrap();
        let got = parse_quote("005930.KS US Equity", now - 2.0, q.v, now).unwrap();
        assert_eq!(got.source.as_deref(), Some("Apple Stocks (delayed)"));
        assert_eq!(got.ts, now as i64 - 1_200);
    }

    #[test]
    fn refresh_batch_size_is_bounded_to_avoid_rapid_stocks_navigation() {
        assert!((2..=4).contains(&MAX_REFRESH_PER_BATCH));
    }

    #[tokio::test]
    async fn simultaneous_refreshes_do_not_navigate_stocks_again() {
        let cache = AppleStocksCache::new("/nonexistent-apple-stocks-cache");
        assert!(!cache.launch_enabled);
        let locked = cache.navigation.lock().await;
        let result = cache.refresh_missing_quotes(&[("AAPL US Equity".into(), "AAPL".into())]).await;
        assert!(result.is_empty(), "a busy Stocks symbol view must not receive a second deep link");
        drop(locked);
        assert!(cache.navigation.try_lock().is_ok());
    }

    #[tokio::test]
    async fn cache_read_mode_does_not_open_stocks_for_missing_symbols() {
        let cache = AppleStocksCache::new("/nonexistent-apple-stocks-cache");
        assert!(!cache.launch_enabled);
        // In particular there must not be a 4-second open/poll wait or an
        // attempt to launch a GUI for either a quote or a chart request.
        let start = Instant::now();
        let quotes = cache.refresh_missing_quotes(&[("AAPL US Equity".into(), "AAPL".into())]).await;
        assert!(quotes.is_empty());
        assert!(start.elapsed() < Duration::from_millis(300));
        assert!(cache.refresh_gate.lock().await.attempted.is_empty());
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
