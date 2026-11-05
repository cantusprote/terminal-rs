//! Live, read-only Kiwoom WebSocket smoke test; no credentials are accessed
//! directly and no account/order operations are permitted.
//!
//! cargo run -p tr-data --example kiwoom_realtime_probe -- 'KOSPI Index'
//! cargo run -p tr-data --example kiwoom_realtime_probe -- '005930.KS'

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let symbol = std::env::args().nth(1).unwrap_or_else(|| "KOSPI Index".into());
    let sec = tr_data::security(&symbol);
    anyhow::ensure!(tr_data::kiwoom::is_domestic(&sec), "Korean symbol required");
    let provider = tr_data::kiwoom::KiwoomProvider::from_env()
        .ok_or_else(|| anyhow::anyhow!("Kiwoom CLI is not installed or is disabled"))?;
    let mut receiver = provider
        .live(&sec, tr_core::Timeframe::M1)
        .ok_or_else(|| anyhow::anyhow!("WebSocket live feed unavailable"))?;
    match tokio::time::timeout(std::time::Duration::from_secs(12), receiver.recv()).await {
        Ok(Some(bar)) => {
            anyhow::ensure!(bar.close.is_finite() && bar.close > 0.0, "invalid observed price");
            println!(
                "KIWOOM_WEBSOCKET_OK symbol={} bar_ts={} last={} volume={}",
                sec.key(),
                bar.ts,
                bar.close,
                bar.volume
            );
        }
        _ => println!(
            "KIWOOM_WEBSOCKET_NO_TRADES symbol={} (outside market hours or feed idle; REST remains available)",
            sec.key()
        ),
    }
    Ok(())
}
