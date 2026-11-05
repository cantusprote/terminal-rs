//! Read-only smoke probe for the local macOS Stocks quote/sparkline cache.
//! Run: TR_APPLE_STOCKS_CACHE=1 cargo run -p tr-data --example apple_stocks_probe
//! End-to-end refresh: TR_APPLE_STOCKS_CACHE=1 cargo run -p tr-data --example apple_stocks_probe -- --route ADBE

#[cfg(target_os = "macos")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use tr_core::Timeframe;
    use tr_data::apple_stocks::AppleStocksCache;

    let Some(cache) = AppleStocksCache::from_env() else {
        anyhow::bail!("Set TR_APPLE_STOCKS_CACHE=1 and open the Stocks app at least once");
    };
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|s| s == "--route") {
        let symbol = args.get(1).ok_or_else(|| anyhow::anyhow!("Usage: --route TICKER"))?;
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let router = tr_data::markets::Markets::new(None)?;
        use tr_data::MarketData;
        let key = format!("{symbol} US Equity");
        let quotes = router.quotes(std::slice::from_ref(&key)).await?;
        let quote = quotes.iter().find(|q| q.symbol == key).ok_or_else(|| anyhow::anyhow!("no price for {key}"))?;
        let apple = cache.quotes(&[(key, symbol.clone())])?;
        println!(
            "router last={} observed_at={} source={} Apple cache available={}",
            quote.last,
            quote.ts,
            quote.source.as_deref().unwrap_or("unknown"),
            !apple.is_empty()
        );
        return Ok(());
    }
    let keys = vec![
        ("AAPL US Equity".into(), "AAPL".into()),
        ("005930.KS US Equity".into(), "005930.KS".into()),
        ("KOSPI Index".into(), "^KS11".into()),
    ];
    for q in cache.quotes(&keys)? {
        println!("{}: last={} previous_close={} timestamp={}", q.symbol, q.last, q.prev_close, q.ts);
    }
    for native in ["AAPL", "005930.KS", "^KS11"] {
        let bars = cache.history(native, Timeframe::M30)?;
        println!("{native}: {} recent 30m OHLCV bars from Apple Stocks cache", bars.len());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("Apple Stocks cache is available on macOS only");
}
