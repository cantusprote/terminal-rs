//! Routes market keys to the free source for each asset class.
//!
//! | class | history | quotes | live |
//! |---|---|---|---|
//! | Crypto | Binance klines (Coinbase fallback) | Binance 24h ticker | Binance kline websocket (Coinbase ticker fallback) |
//! | Govt | Treasury.gov daily curve (FRED fallback); Yahoo `^TNX` etc. intraday | Treasury.gov | — |
//! | Equity/ETF | Alpaca if keyed, else Yahoo | Yahoo spark | — |
//! | FX/Index/Comdty | Yahoo (`EURUSD=X`, `^GSPC`, `CL=F`) | Yahoo spark | — |
//! | HIP-3 perps (`xyz:AAPL`) | — | — | — |
//!
//! HIP-3 perps come only from the Terminal server ([`crate::remote::RemoteMarkets`]).

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use tr_core::{AssetClass, Bar, Security, Timeframe, catalog};

use crate::alpaca::AlpacaProvider;
#[cfg(target_os = "macos")]
use crate::apple_stocks::AppleStocksCache;
use crate::crypto::CryptoProvider;
use crate::treasury::TreasuryProvider;
use crate::yahoo::{YahooProvider, yahoo_symbol};
use crate::{
    LiveFeed, MarketData, NewsItem, OptionChain, OrderBook, Quote, SearchHit, YieldCurves, perp_needs_server, security,
};

pub struct Markets {
    yahoo: YahooProvider,
    crypto: CryptoProvider,
    treasury: TreasuryProvider,
    alpaca: Option<AlpacaProvider>,
    #[cfg(target_os = "macos")]
    apple: Option<AppleStocksCache>,
    name: String,
}

#[cfg(target_os = "macos")]
fn apple_supports(class: AssetClass) -> bool {
    matches!(class, AssetClass::Equity | AssetClass::Etf | AssetClass::Index)
}

impl Markets {
    pub fn new(alpaca: Option<AlpacaProvider>) -> Result<Self> {
        let name = if alpaca.is_some() { "alpaca+yahoo+binance+ust" } else { "yahoo+binance+ust" }.to_string();
        #[cfg(target_os = "macos")]
        let apple = AppleStocksCache::from_env();
        #[cfg(target_os = "macos")]
        let name = if apple.is_some() { format!("apple-stocks-cache+{name}") } else { name };
        Ok(Self {
            yahoo: YahooProvider::new()?,
            crypto: CryptoProvider::new()?,
            treasury: TreasuryProvider::new()?,
            alpaca,
            #[cfg(target_os = "macos")]
            apple,
            name,
        })
    }
}

#[async_trait]
impl MarketData for Markets {
    fn name(&self) -> &str {
        &self.name
    }

    async fn history(&self, symbol: &str, tf: Timeframe, end: Option<i64>, limit: usize) -> Result<Vec<Bar>> {
        let sec = security(symbol);
        // A chart may prime Apple's cache only when the separate,
        // explicitly authorized Stocks auto-launch setting is enabled.
        // Cache-only mode never opens Stocks and uses Yahoo/Alpaca instead.
        #[cfg(target_os = "macos")]
        if end.is_none()
            && apple_supports(sec.class)
            && let Some(cache) = &self.apple
        {
            let native = yahoo_symbol(&sec);
            let lookup = [(symbol.to_owned(), native)];
            if cache.quotes(&lookup).is_ok_and(|quotes| quotes.is_empty()) {
                let _ = cache.refresh_missing_quotes(&lookup).await;
            }
        }
        let primary = match sec.class {
            AssetClass::Perp => Err(perp_needs_server(symbol)),
            AssetClass::Crypto => self.crypto.history(&sec, tf, end, limit).await,
            AssetClass::Govt if tf.is_intraday() => match catalog::by_security(&sec).and_then(|l| l.yahoo) {
                Some(y) => self.yahoo.history(y, tf, end, limit).await,
                None => bail!("intraday yields are not available for {}; use D, W or M", sec.ticker),
            },
            AssetClass::Govt => self.treasury.history(&sec, tf, end, limit).await,
            AssetClass::Equity | AssetClass::Etf if self.alpaca.is_some() => {
                let a = self.alpaca.as_ref().expect("checked");
                match a.history(&sec.ticker, tf, end, limit).await {
                    Ok(b) if !b.is_empty() => Ok(b),
                    _ => self.yahoo.history(&yahoo_symbol(&sec), tf, end, limit).await,
                }
            }
            _ => self.yahoo.history(&yahoo_symbol(&sec), tf, end, limit).await,
        };
        // Apple Stocks only keeps a short recent sparkline, generally with 10m
        // OHLCV. Merge its latest compatible 30m/1h/4h buckets with the normal
        // full historical source; never use it for older-history paging.
        #[cfg(target_os = "macos")]
        if end.is_none()
            && apple_supports(sec.class)
            && let Some(cache) = &self.apple
        {
            match cache.history(&yahoo_symbol(&sec), tf) {
                Ok(recent) if !recent.is_empty() => {
                    let mut bars = match primary {
                        Ok(bars) => bars,
                        Err(e) => {
                            tracing::warn!(
                                "history {symbol}: network source unavailable ({e:#}); using short Apple Stocks cache"
                            );
                            Vec::new()
                        }
                    };
                    let last = bars.last().map(|b| b.ts);
                    // Use Apple only for the most recent bucket(s); do not
                    // overwrite older, already complete bars from the feed.
                    for bar in recent {
                        if last.is_none_or(|ts| bar.ts >= ts) {
                            match bars.binary_search_by_key(&bar.ts, |b| b.ts) {
                                Ok(i) => bars[i] = bar,
                                Err(i) => bars.insert(i, bar),
                            }
                        }
                    }
                    return Ok(crate::clip(bars, end, limit));
                }
                Err(e) => tracing::warn!("Apple Stocks sparkline unavailable for {symbol}: {e:#}"),
                _ => {}
            }
        }
        primary
    }

    async fn quotes(&self, symbols: &[String]) -> Result<Vec<Quote>> {
        let (mut crypto, mut govt, mut other) = (Vec::new(), Vec::new(), Vec::new());
        for key in symbols {
            let sec = security(key);
            match sec.class {
                AssetClass::Perp => {}
                AssetClass::Crypto => crypto.push((key.clone(), sec)),
                AssetClass::Govt => govt.push((key.clone(), sec)),
                _ => other.push((key.clone(), sec)),
            }
        }
        // Read Apple's local cache first, with Yahoo only for symbols that are
        // absent, stale, or malformed. No XPC calls or auth tokens are involved.
        #[cfg(target_os = "macos")]
        let mut apple_quotes: Vec<Quote> = Vec::new();
        #[cfg(not(target_os = "macos"))]
        let apple_quotes: Vec<Quote> = Vec::new();
        #[cfg(target_os = "macos")]
        if let Some(cache) = &self.apple {
            let pairs: Vec<(String, String)> = other
                .iter()
                .filter(|(_, sec)| apple_supports(sec.class))
                .map(|(key, sec)| (key.clone(), yahoo_symbol(sec)))
                .collect();
            match cache.quotes(&pairs) {
                Ok(quotes) => apple_quotes = quotes,
                Err(e) => tracing::warn!("Apple Stocks cache unavailable: {e:#}"),
            }
            let missing: Vec<(String, String)> =
                pairs.into_iter().filter(|(key, _)| !apple_quotes.iter().any(|q| q.symbol == *key)).collect();
            if !missing.is_empty() {
                apple_quotes.extend(cache.refresh_missing_quotes(&missing).await);
            }
        }
        let remaining: Vec<_> =
            other.iter().filter(|(key, _)| !apple_quotes.iter().any(|q| q.symbol == *key)).collect();
        let (c, g, y) = tokio::join!(
            async { if crypto.is_empty() { Ok(vec![]) } else { self.crypto.quotes(&crypto).await } },
            async { if govt.is_empty() { Ok(vec![]) } else { self.treasury.quotes(&govt).await } },
            async {
                if remaining.is_empty() {
                    return Ok(vec![]);
                }
                let ys: Vec<String> = remaining.iter().map(|(_, s)| yahoo_symbol(s)).collect();
                let quotes = self.yahoo.quotes(&ys).await?;
                // Map Yahoo symbols back to the caller's market keys.
                Ok::<_, anyhow::Error>(
                    remaining
                        .iter()
                        .zip(&ys)
                        .filter_map(|((key, _), y)| {
                            quotes.iter().find(|q| &q.symbol == y).map(|q| Quote { symbol: key.clone(), ..q.clone() })
                        })
                        .collect(),
                )
            }
        );
        let mut out = apple_quotes;
        let mut errors = Vec::new();
        for r in [c, g, y] {
            match r {
                Ok(q) => out.extend(q),
                Err(e) => errors.push(format!("{e:#}")),
            }
        }
        if out.is_empty() && !errors.is_empty() {
            return Err(anyhow!(errors.join("; ")));
        }
        Ok(out)
    }

    fn live(&self, symbol: &str, tf: Timeframe) -> Option<LiveFeed> {
        let sec = security(symbol);
        (sec.class == AssetClass::Crypto).then(|| CryptoProvider::live(&sec, tf))
    }

    async fn search(&self, query: &str) -> Result<Vec<SearchHit>> {
        self.yahoo.search(query).await
    }

    async fn news(&self, symbol: Option<&str>) -> Result<Vec<NewsItem>> {
        let q = match symbol.map(security) {
            Some(sec) => match sec.class {
                // Yahoo has no crypto headlines; use CoinDesk's feed.
                AssetClass::Crypto => return self.crypto.news(&sec).await,
                // A linked perp shares its underlying's headlines.
                AssetClass::Perp => match tr_core::perps::link_for(&sec.ticker) {
                    Some(l) => yahoo_symbol(&security(l.underlying)),
                    None => return Ok(Vec::new()),
                },
                AssetClass::Govt => catalog::by_security(&sec).and_then(|l| l.yahoo).unwrap_or("^TNX").to_string(),
                _ => yahoo_symbol(&sec),
            },
            None => "stock market".into(),
        };
        self.yahoo.news(&q).await
    }

    /// Listed options on US equities, ETFs and indices (Yahoo).
    async fn options(&self, symbol: &str, expiration: Option<i64>) -> Result<OptionChain> {
        let sec = security(symbol);
        match sec.class {
            AssetClass::Equity | AssetClass::Etf | AssetClass::Index => {
                let y = yahoo_symbol(&sec);
                // Yahoo lists S&P 500 index options under ^SPX, not the ^GSPC quote symbol.
                let y = if y == "^GSPC" { "^SPX".to_string() } else { y };
                self.yahoo.options(&y, expiration).await
            }
            _ => bail!("listed options are available for US equities, ETFs and indices, not {}", sec.key()),
        }
    }

    async fn order_book(&self, symbol: &str, depth: usize) -> Result<OrderBook> {
        let sec = security(symbol);
        match sec.class {
            AssetClass::Crypto => self.crypto.order_book(symbol, &sec, depth).await,
            _ => bail!("no free public order book for {}", sec.key()),
        }
    }

    async fn yield_curves(&self) -> Result<YieldCurves> {
        self.treasury.curves().await
    }
}

/// Market key for a security (convenience for callers holding a [`Security`]).
pub fn key(sec: &Security) -> String {
    sec.key()
}
