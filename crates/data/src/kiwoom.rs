//! Domestic Korean stock/index market data via Kiwoom's official REST/WebSocket CLI.
//!
//! This adapter runs only allowlisted, **read-only** `kiwoomcli domestic`
//! operations. The CLI reuses credentials and tokens already stored in the
//! user's macOS Keychain. No key, token, order, account, or execution operation
//! enters Terminal-rs, its MCP server, or its source tree.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{FixedOffset, NaiveDate, NaiveDateTime, Utc};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, mpsc};
use tr_core::{AssetClass, Bar, Security, Timeframe};

use crate::{LiveFeed, OrderBook, Quote, SearchHit};

const RATE_SPACING: Duration = Duration::from_millis(280); // <= 4 REST requests/s
const MAX_KOREAN_SEARCH: usize = 80;

#[derive(Clone)]
pub struct KiwoomProvider {
    cli: PathBuf,
    profile: Option<String>,
    gate: Arc<Mutex<Instant>>,
    catalog: Arc<Mutex<Option<Vec<SearchHit>>>>,
}

/// Index codes are from Kiwoom's sector API, not Yahoo's market keys.
pub fn domestic_index(symbol: &str) -> Option<&'static str> {
    match symbol.to_ascii_uppercase().as_str() {
        "KOSPI" | "^KS11" => Some("001"),
        "KOSDAQ" | "^KQ11" => Some("101"),
        "KOSPI200" | "^KS200" => Some("201"),
        _ => None,
    }
}

pub fn domestic_stock_code(symbol: &str) -> Option<&str> {
    let upper = symbol.to_ascii_uppercase();
    let qualified = upper.strip_suffix(".KS").or_else(|| upper.strip_suffix(".KQ"));
    let code = qualified.unwrap_or(&upper);
    if code.len() == 6 && code.bytes().all(|c| c.is_ascii_digit())
        || qualified.is_some()
            && code.len() == 6
            && code.bytes().all(|c| c.is_ascii_alphanumeric())
            && code.bytes().any(|c| c.is_ascii_digit())
    {
        // Return a slice of the original string, preserving its byte boundaries.
        Some(&symbol[..6])
    } else {
        None
    }
}

pub fn is_domestic(sec: &Security) -> bool {
    if domestic_index(&sec.ticker).is_some() {
        return matches!(sec.class, AssetClass::Index | AssetClass::Equity);
    }
    match sec.class {
        AssetClass::Equity | AssetClass::Etf => domestic_stock_code(&sec.ticker).is_some(),
        _ => false,
    }
}

impl KiwoomProvider {
    /// No credential reads: check only that the official CLI is executable.
    /// Explicit TR_KIWOOM_ENABLED=0 disables domestic routing.
    pub fn from_env() -> Option<Self> {
        if std::env::var("TR_KIWOOM_ENABLED").ok().is_some_and(|v| matches!(v.as_str(), "0" | "false")) {
            return None;
        }
        let cli = std::env::var_os("TR_KIWOOM_CLI")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/bin/kiwoomcli")))?;
        if !cli.is_file() {
            tracing::warn!(
                "Kiwoom CLI not installed at {}; Korean instruments will use legacy providers",
                cli.display()
            );
            return None;
        }
        let profile = std::env::var("TR_KIWOOM_PROFILE").ok().filter(|v| !v.is_empty());
        Some(Self {
            cli,
            profile,
            gate: Arc::new(Mutex::new(Instant::now() - RATE_SPACING)),
            catalog: Arc::new(Mutex::new(None)),
        })
    }

    fn command(&self, parts: &[&str]) -> Command {
        let mut cmd = Command::new(&self.cli);
        cmd.arg("domestic").args(parts).arg("--format").arg("json");
        if let Some(profile) = &self.profile {
            cmd.arg("--profile").arg(profile);
        }
        cmd.kill_on_drop(true);
        cmd
    }

    async fn rest(&self, parts: &[&str]) -> Result<Value> {
        // Keep below the official 5 REST queries/second cap for this CLI
        // process. CLI auth/token refreshing stays in its own Keychain store.
        {
            let mut last = self.gate.lock().await;
            let wait = RATE_SPACING.saturating_sub(last.elapsed());
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
            *last = Instant::now();
        }
        let output = tokio::time::timeout(Duration::from_secs(25), self.command(parts).output())
            .await
            .context("Kiwoom read-only REST request timed out")?
            .context("launch official Kiwoom CLI")?;
        if !output.status.success() {
            // The CLI could print credential context on stderr. Never include
            // its raw text in a GUI status, MCP reply or application log.
            bail!("Kiwoom {} request failed (exit status {})", parts.join("/"), output.status);
        }
        let body: Value = serde_json::from_slice(&output.stdout).context("decode Kiwoom REST response JSON")?;
        if body.get("return_code").and_then(Value::as_i64).is_some_and(|n| n != 0) {
            bail!("Kiwoom {} returned an API error", parts.join("/"));
        }
        Ok(body)
    }

    pub async fn quotes(&self, securities: &[(String, Security)]) -> Result<Vec<Quote>> {
        let mut result = Vec::new();
        let mut failures = 0;
        for (key, sec) in securities {
            let payload = if let Some(code) = domestic_index(&sec.ticker) {
                let market = match code {
                    "001" => "kospi",
                    "101" => "kosdaq",
                    _ => "kospi200",
                };
                self.rest(&["sectors", "price", "--market", market, "--code", code])
                    .await
                    .and_then(|v| parse_quote(&v, key, true))
            } else if let Some(code) = domestic_stock_code(&sec.ticker) {
                self.rest(&["quotes", "price", "--code", code]).await.and_then(|v| parse_quote(&v, key, false))
            } else {
                continue;
            };
            match payload {
                Ok(quote) => result.push(quote),
                Err(error) => {
                    failures += 1;
                    tracing::warn!("Kiwoom market quote {} unavailable: {error:#}", key);
                }
            }
        }
        if failures > 0 && result.is_empty() {
            bail!("Kiwoom quotes unavailable for {failures} Korean instruments");
        }
        Ok(result)
    }

    pub async fn history(&self, sec: &Security, tf: Timeframe, end: Option<i64>, limit: usize) -> Result<Vec<Bar>> {
        let index = domestic_index(&sec.ticker).is_some();
        let code = domestic_index(&sec.ticker)
            .or_else(|| domestic_stock_code(&sec.ticker))
            .ok_or_else(|| anyhow!("not a Korean market instrument: {}", sec.ticker))?;
        let tf_arg = match tf {
            Timeframe::M1 => "1",
            Timeframe::M5 => "5",
            Timeframe::M15 => "15",
            Timeframe::M30 => "30",
            Timeframe::H1 | Timeframe::H4 => "60",
            _ => "",
        };
        let date = chrono::DateTime::from_timestamp(end.unwrap_or_else(|| Utc::now().timestamp()), 0)
            .unwrap_or_else(Utc::now)
            .with_timezone(&korea())
            .format("%Y%m%d")
            .to_string();
        let is_intraday = tf.is_intraday();
        let label = if index {
            match tf {
                Timeframe::W1 => "sector-weekly",
                Timeframe::MN1 => "sector-monthly",
                _ if is_intraday => "sector-minute",
                _ => "sector-daily",
            }
        } else {
            match tf {
                Timeframe::W1 => "stock-weekly",
                Timeframe::MN1 => "stock-monthly",
                _ if is_intraday => "stock-minute",
                _ => "daily",
            }
        };
        let payload = if is_intraday {
            self.rest(&["candles", label, "--code", code, "--interval", tf_arg]).await?
        } else {
            self.rest(&["candles", label, "--code", code, "--date", &date]).await?
        };
        let rows = [
            "stk_dt_pole_chart_qry",
            "stk_min_pole_chart_qry",
            "stk_stk_pole_chart_qry",
            "stk_mth_pole_chart_qry",
            "inds_dt_pole_qry",
            "inds_min_pole_qry",
            "inds_stk_pole_qry",
            "inds_mth_pole_qry",
        ]
        .into_iter()
        .find_map(|k| payload.get(k).and_then(Value::as_array));
        let Some(rows) = rows else {
            bail!("Kiwoom {} returned no recognized historical series", label);
        };
        let mut bars: Vec<Bar> = rows.iter().filter_map(|r| parse_bar(r, is_intraday, index)).collect();
        bars.sort_by_key(|b| b.ts);
        bars.dedup_by_key(|b| b.ts);
        if tf == Timeframe::H4 {
            bars = aggregate(bars, tf);
        }
        if let Some(cutoff) = end {
            bars.retain(|b| b.ts < cutoff);
        }
        let skip = bars.len().saturating_sub(limit);
        Ok(bars.into_iter().skip(skip).collect())
    }

    pub async fn order_book(&self, sec: &Security, depth: usize) -> Result<OrderBook> {
        let code = domestic_stock_code(&sec.ticker).ok_or_else(|| anyhow!("index order book unavailable"))?;
        let payload = self.rest(&["orderbooks", "list", "--code", code]).await?;
        let mut bids = Vec::new();
        let mut asks = Vec::new();
        for level in 1..=depth.min(10) {
            let sell = format!("sel_{level}th_pre_bid");
            let sell_qty = format!("sel_{level}th_pre_req");
            let buy = format!("buy_{level}th_pre_bid");
            let buy_qty = format!("buy_{level}th_pre_req");
            if let (Some(price), Some(qty)) = (number(&payload[&sell]), number(&payload[&sell_qty]))
                && price > 0.0
                && qty > 0.0
            {
                asks.push((price, qty));
            }
            if let (Some(price), Some(qty)) = (number(&payload[&buy]), number(&payload[&buy_qty]))
                && price > 0.0
                && qty > 0.0
            {
                bids.push((price, qty));
            }
        }
        if bids.is_empty() && asks.is_empty() {
            bail!("Kiwoom order book empty (market may be closed)");
        }
        Ok(OrderBook { symbol: sec.key(), bids, asks, source: "Kiwoom REST".into(), simulated: false })
    }

    pub async fn search(&self, query: &str) -> Result<Vec<SearchHit>> {
        let q = query.trim();
        let index_hits: Vec<SearchHit> = ["KOSPI", "KOSDAQ", "KOSPI200"]
            .into_iter()
            .filter(|name| name.contains(&q.to_ascii_uppercase()))
            .map(|name| SearchHit {
                key: format!("{name} Index"),
                name: format!("{name} Index"),
                class: AssetClass::Index,
                exchange: Some("KRX".into()),
            })
            .collect();
        if q.is_empty() || (!q.chars().any(|c| ('가'..='힣').contains(&c)) && !q.chars().any(|c| c.is_ascii_digit()))
        {
            return Ok(index_hits);
        }
        let mut guard = self.catalog.lock().await;
        if guard.is_none() {
            let mut all = Vec::new();
            for (market, suffix, class) in [
                ("kospi", ".KS", AssetClass::Equity),
                ("kosdaq", ".KQ", AssetClass::Equity),
                ("etf", ".KS", AssetClass::Etf),
            ] {
                match self.rest(&["stocks", "info-list", "--market-type", market]).await {
                    Ok(body) => {
                        if let Some(items) = body["list"].as_array() {
                            for item in items {
                                let (Some(code), Some(name)) = (item["code"].as_str(), item["name"].as_str()) else {
                                    continue;
                                };
                                if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_alphanumeric()) {
                                    continue;
                                }
                                all.push(SearchHit {
                                    key: format!("{code}{suffix} US Equity"),
                                    name: name.into(),
                                    class,
                                    exchange: Some(market.to_uppercase()),
                                });
                            }
                        }
                    }
                    Err(error) => tracing::warn!("Kiwoom stock-list {market} unavailable: {error:#}"),
                }
            }
            *guard = Some(all);
        }
        let query_up = q.to_uppercase();
        let mut hits = index_hits;
        if let Some(all) = guard.as_ref() {
            hits.extend(
                all.iter()
                    .filter(|item| item.name.to_uppercase().contains(&query_up) || item.key.contains(&query_up))
                    .take(MAX_KOREAN_SEARCH)
                    .cloned(),
            );
        }
        Ok(hits)
    }

    /// Stream-only market data: one subscribed chart per consumer. This cannot
    /// submit orders and dies when the GUI's chart receiver is dropped.
    pub fn live(&self, sec: &Security, tf: Timeframe) -> Option<LiveFeed> {
        let (code, stream) = if let Some(code) = domestic_index(&sec.ticker) {
            (code.to_string(), "sector-index")
        } else {
            (domestic_stock_code(&sec.ticker)?.to_string(), "trades")
        };
        let cli = self.cli.clone();
        let profile = self.profile.clone();
        let (sender, receiver) = mpsc::channel(128);
        tokio::spawn(async move {
            loop {
                let mut process = Command::new(&cli);
                process
                    .args(["domestic", "streams", stream, "--code", &code, "--watch", "--format", "jsonl"])
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null()) // no token/context leakage
                    .kill_on_drop(true);
                if let Some(profile) = &profile {
                    process.args(["--profile", profile]);
                }
                let Ok(mut child) = process.spawn() else {
                    tracing::warn!("Kiwoom realtime process cannot start");
                    break;
                };
                let Some(stdout) = child.stdout.take() else { break };
                let mut lines = BufReader::new(stdout).lines();
                let mut active: Option<Bar> = None;
                loop {
                    tokio::select! {
                        () = sender.closed() => break,
                        line = lines.next_line() => match line {
                            Ok(Some(message)) => {
                                if message.len() > 256_000 { continue; }
                                if let Ok(frame) = serde_json::from_str::<Value>(&message) {
                                    for row in frame["data"].as_array().into_iter().flatten() {
                                        if let Some(bar) = parse_stream_bar(row, &code, tf, active) {
                                            active = Some(bar);
                                            if sender.send(bar).await.is_err() { break; }
                                        }
                                    }
                                }
                            }
                            Ok(None) | Err(_) => break,
                        }
                    }
                }
                let _ = child.start_kill();
                let _ = child.wait().await;
                if sender.is_closed() {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        });
        Some(receiver)
    }
}

fn korea() -> FixedOffset {
    FixedOffset::east_opt(9 * 3600).expect("fixed KST offset")
}

/// Signed Kiwoom price fields encode price direction; prices themselves
/// are nonnegative. Only explicit price fields use this conversion.
fn number(v: &Value) -> Option<f64> {
    let raw = v.as_str().map(str::to_owned).or_else(|| v.as_f64().map(|n| n.to_string()))?;
    let clean: String = raw.trim().trim_start_matches(&['+', '-'][..]).replace(',', "");
    let n = clean.parse::<f64>().ok()?;
    n.is_finite().then_some(n)
}

fn signed(v: &Value) -> Option<f64> {
    let s = v.as_str()?;
    let magnitude = number(v)?;
    Some(if s.trim_start().starts_with('-') { -magnitude } else { magnitude })
}

fn parse_kst_stamp(date: &str, time: &str) -> Option<i64> {
    let dt = NaiveDateTime::parse_from_str(&format!("{date}{time}"), "%Y%m%d%H%M%S").ok()?;
    dt.and_local_timezone(korea()).single().map(|x| x.timestamp())
}

fn parse_quote(v: &Value, symbol: &str, index: bool) -> Result<Quote> {
    let price = number(&v["cur_prc"]).filter(|n| *n > 0.0).ok_or_else(|| anyhow!("missing current price"))?;
    let prev = if index {
        price - signed(&v["pred_pre"]).ok_or_else(|| anyhow!("missing index previous-close delta"))?
    } else {
        number(&v["pred_close_pric"]).filter(|p| *p > 0.0).ok_or_else(|| anyhow!("missing stock previous close"))?
    };
    let kst_now = Utc::now().with_timezone(&korea());
    let date = v["date"].as_str().unwrap_or("");
    let date = if date.len() == 8 { date.to_string() } else { kst_now.format("%Y%m%d").to_string() };
    let ts = if index {
        // Kiwoom sometimes puts "888888" (an invalid sentinel) ahead of the
        // latest real index observation after the exchange closes. Scan the
        // descending observations rather than treating that sentinel as
        // a missing timestamp or inventing the current request time.
        v["inds_cur_prc_tm"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| row["tm_n"].as_str())
            .find_map(|time| parse_kst_stamp(&date, time))
            .unwrap_or_default()
    } else {
        v["tm"].as_str().and_then(|time| parse_kst_stamp(&date, time)).unwrap_or_default()
    };
    // If the provider omits a genuine observation time, use 0 (unknown)
    // instead of misrepresenting 'request time' as a trade timestamp.
    Ok(Quote {
        symbol: symbol.into(),
        last: price,
        prev_close: prev,
        ts,
        spark: vec![],
        source: Some("Kiwoom REST".into()),
    })
}

fn parse_bar(row: &Value, intraday: bool, index: bool) -> Option<Bar> {
    let date = if intraday { row["cntr_tm"].as_str()? } else { row["dt"].as_str()? };
    let ts = if intraday {
        let dt = NaiveDateTime::parse_from_str(date, "%Y%m%d%H%M%S").ok()?;
        dt.and_local_timezone(korea()).single()?.timestamp()
    } else {
        let dt = NaiveDate::parse_from_str(date, "%Y%m%d").ok()?.and_hms_opt(9, 0, 0)?;
        dt.and_local_timezone(korea()).single()?.timestamp()
    };
    let scale = if index { 100.0 } else { 1.0 };
    let open = number(&row["open_pric"])? / scale;
    let high = number(&row["high_pric"])? / scale;
    let low = number(&row["low_pric"])? / scale;
    let close = number(&row["cur_prc"])? / scale;
    let volume = number(&row["trde_qty"]).unwrap_or_default() * if index { 1000.0 } else { 1.0 };
    if open <= 0.0 || high < low || close <= 0.0 {
        return None;
    }
    Some(Bar::new(ts, open, high, low, close, volume))
}

fn aggregate(bars: Vec<Bar>, tf: Timeframe) -> Vec<Bar> {
    let mut result: Vec<Bar> = Vec::new();
    for bar in bars {
        let bucket = tf.bucket_start(bar.ts);
        if let Some(last) = result.last_mut()
            && last.ts == bucket
        {
            last.high = last.high.max(bar.high);
            last.low = last.low.min(bar.low);
            last.close = bar.close;
            last.volume += bar.volume;
        } else {
            result.push(Bar { ts: bucket, ..bar });
        }
    }
    result
}

fn parse_stream_bar(row: &Value, code: &str, tf: Timeframe, previous: Option<Bar>) -> Option<Bar> {
    let kind = if code.len() == 3 { "0J" } else { "0B" };
    if row["type"].as_str()? != kind || row["item"].as_str()? != code {
        return None;
    }
    let values = &row["values"];
    let time = values["20"].as_str()?;
    let kst_now = Utc::now().with_timezone(&korea());
    let date = kst_now.format("%Y%m%d").to_string();
    let ts = tf.bucket_start(parse_kst_stamp(&date, time)?);
    let close = number(&values["10"])?;
    if close <= 0.0 {
        return None;
    }
    let open = previous.filter(|b| b.ts == ts).map_or(close, |b| b.open);
    let high = previous.filter(|b| b.ts == ts).map_or(close, |b| b.high.max(close));
    let low = previous.filter(|b| b.ts == ts).map_or(close, |b| b.low.min(close));
    let traded = number(&values["15"]).unwrap_or_default() * if kind == "0J" { 1000.0 } else { 1.0 };
    let volume = previous.filter(|b| b.ts == ts).map_or(traded, |b| b.volume + traded);
    Some(Bar::new(ts, open, high, low, close, volume))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn routes_only_korean_instruments() {
        for s in [
            "005930.KS",
            "035720.KQ",
            "005930",
            "00088K.KS",
            "0048K0.KS",
            "0038A0.KS",
            "KOSPI Index",
            "KOSDAQ Index",
            "KOSPI200 Index",
            "^KS11",
            "^KQ11",
        ] {
            assert!(is_domestic(&crate::security(s)), "{s}");
        }
        for s in ["AAPL US Equity", "SPX Index", "BTCUSD Curncy", "USDJPY Curncy", "GC1 Comdty", "xyz:KR200"] {
            assert!(!is_domestic(&crate::security(s)), "{s}");
        }
    }

    #[test]
    fn interprets_direction_signs_and_actual_trade_time() {
        let s = json!({"cur_prc":"-201000","pred_close_pric":"218000","date":"20261105","tm":"151537"});
        let q = parse_quote(&s, "005930.KS US Equity", false).unwrap();
        assert_eq!((q.last, q.prev_close), (201000.0, 218000.0));
        assert_eq!(q.ts, parse_kst_stamp("20261105", "151537").unwrap());
        assert_eq!(signed(&json!("--310.29")), Some(-310.29));
        let i = json!({"cur_prc":"-5371.46","pred_pre":"-312.34","inds_cur_prc_tm":[{"tm_n":"151350"}]});
        let q = parse_quote(&i, "KOSPI Index", true).unwrap();
        assert!((q.prev_close - 5683.8).abs() < 0.01);
        assert_eq!(q.source.as_deref(), Some("Kiwoom REST"));
        let after_close = json!({"cur_prc":"-5383.80","pred_pre":"-300.00",
            "inds_cur_prc_tm":[{"tm_n":"888888"},{"tm_n":"153220"}]});
        let q = parse_quote(&after_close, "KOSPI Index", true).unwrap();
        let today = Utc::now().with_timezone(&korea()).format("%Y%m%d").to_string();
        assert_eq!(q.ts, parse_kst_stamp(&today, "153220").unwrap());
    }

    #[test]
    fn parses_stock_and_index_history_with_correct_prices() {
        let stock = json!({"cur_prc":"-201250","open_pric":"-201000","high_pric":"-201500",
            "low_pric":"-200500","trde_qty":"2300","cntr_tm":"20261105151400"});
        let s = parse_bar(&stock, true, false).unwrap();
        assert_eq!(s.close, 201250.0);
        let ix = json!({"cur_prc":"-537351","open_pric":"+537108","high_pric":"+537363",
            "low_pric":"+537108","trde_qty":"312","cntr_tm":"20261105151400"});
        let i = parse_bar(&ix, true, true).unwrap();
        assert!((i.close - 5373.51).abs() < 0.0001);
        assert_eq!(i.volume, 312000.0);
    }

    #[test]
    fn decodes_realtime_frames_for_both_asset_types() {
        let s = json!({"type":"0B","item":"005930","values":{"20":"151539","10":"-201000","15":"-335"}});
        let b = parse_stream_bar(&s, "005930", Timeframe::M1, None).unwrap();
        assert_eq!(b.close, 201000.0);
        let i = json!({"type":"0J","item":"001","values":{"20":"151700","10":"-5386.47","15":"116"}});
        assert_eq!(parse_stream_bar(&i, "001", Timeframe::M1, None).unwrap().close, 5386.47);
        assert!(parse_stream_bar(&s, "000660", Timeframe::M1, None).is_none());
    }
}
