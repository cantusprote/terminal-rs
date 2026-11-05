//! Standalone CLI entry point for local Terminal-rs data and app control.
//! All outputs are machine-readable JSON. No brokerage trading methods exist.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde_json::{Value, json};
use tr_control::{Command, MAX_MESSAGE_BYTES, Reply, socket_path, valid_expression, valid_symbol};
use tr_core::Timeframe;

const HELP: &str = r#"Terminal-rs CLI (personal, read-only)
  terminal-cli doctor
  terminal-cli quote SYMBOL [SYMBOL...]
  terminal-cli history SYMBOL [TIMEFRAME=D1] [LIMIT=50]
  terminal-cli search TERMS...
  terminal-cli book SYMBOL [DEPTH=20]
  terminal-cli app status
  terminal-cli app watchlist
  terminal-cli app add SYMBOL
  terminal-cli app remove SYMBOL
  terminal-cli app screen EXPRESSION...
  terminal-cli app alert SYMBOL ABOVE|BELOW PRICE
  terminal-cli app technical-alert SYMBOL VOL 2 | RSI ABOVE 70 | SMA BELOW
  terminal-cli app chart SYMBOL
  terminal-cli app quotes SYMBOL [SYMBOL...]
  No order placement, order cancellation, execution, wallet signing,
  arbitrary shell commands or source-file editing.
"#;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || matches!(args[0].as_str(), "--help" | "-h" | "help") {
        println!("{HELP}");
        return;
    }
    match execute(&args).await {
        Ok(data) => println!("{}", serde_json::to_string(&json!({"ok":true,"data":data})).expect("valid JSON")),
        Err(error) => {
            println!("{}", serde_json::to_string(&json!({"ok":false,"error":error})).expect("valid JSON"));
            std::process::exit(1);
        }
    }
}

fn checked_symbol(input: &str) -> Result<String, String> {
    if !valid_symbol(input) {
        return Err("invalid symbol: expected a market key of no more than 96 plain characters".into());
    }
    Ok(tr_data::security(input).key())
}

/// Match the MCP timeframe enum and the chart's human aliases.
fn parse_timeframe(text: &str) -> Result<Timeframe, String> {
    match text.to_ascii_uppercase().as_str() {
        "M1" => Ok(Timeframe::M1),
        "M5" => Ok(Timeframe::M5),
        "M15" => Ok(Timeframe::M15),
        "M30" => Ok(Timeframe::M30),
        "H1" => Ok(Timeframe::H1),
        "H4" => Ok(Timeframe::H4),
        "D1" => Ok(Timeframe::D1),
        "W1" => Ok(Timeframe::W1),
        "MN1" => Ok(Timeframe::MN1),
        _ => text.parse::<Timeframe>().map_err(|_| format!("invalid timeframe: {text}")),
    }
}

async fn execute(args: &[String]) -> Result<Value, String> {
    let head = args[0].as_str();
    if head == "doctor" && args.len() == 1 {
        let path = socket_path();
        let running = UnixStream::connect(&path).is_ok();
        return Ok(json!({
            "app_running":running,
            "socket":path.to_string_lossy(),
            "read_only":true,
            "trade_execution":false,
            "apple_stocks_opt_in":std::env::var("TR_APPLE_STOCKS_CACHE").ok()
                .is_some_and(|s| s == "1" || s.eq_ignore_ascii_case("true")),
        }));
    }
    if head == "app" {
        let Some((op, rest)) = args[1..].split_first() else {
            return Err(
                "app action required: status/watchlist/add/remove/screen/alert/technical-alert/chart/quotes".into()
            );
        };
        let one = || rest.first().ok_or_else(|| format!("{op} requires a symbol"));
        let expression = || {
            let s = rest.join(" ");
            if !valid_expression(&s) {
                Err("expected a printable expression up to 256 characters".to_string())
            } else {
                Ok(s)
            }
        };
        let command = match op.as_str() {
            "status" if rest.is_empty() => Command::Status,
            "watchlist" if rest.is_empty() => Command::Watchlist,
            "add" if rest.len() == 1 => Command::WatchAdd { symbol: checked_symbol(one()?)? },
            "remove" if rest.len() == 1 => Command::WatchRemove { symbol: checked_symbol(one()?)? },
            "screen" => Command::Screen { expression: expression()? },
            "alert" => Command::PriceAlert { expression: expression()? },
            "technical-alert" => Command::TechnicalAlert { expression: expression()? },
            "chart" if rest.len() == 1 => Command::OpenChart { symbol: checked_symbol(one()?)? },
            "quotes" if (1..=50).contains(&rest.len()) => {
                Command::Quotes { symbols: rest.iter().map(|s| checked_symbol(s)).collect::<Result<_, _>>()? }
            }
            _ => return Err(format!("unsupported app action: {op}; use --help")),
        };
        return app_call(command);
    }
    let provider = tr_data::from_env();
    match head {
        "quote" if (2..=21).contains(&args.len()) => {
            let symbols: Vec<String> = args[1..].iter().map(|s| checked_symbol(s)).collect::<Result<_, _>>()?;
            let name = provider.name().to_string();
            let quotes = provider.quotes(&symbols).await.map_err(|e| format!("{e:#}"))?;
            Ok(json!({"provider":name,"requested":symbols,"quotes":quotes}))
        }
        "history" if (2..=4).contains(&args.len()) => {
            let symbol = checked_symbol(&args[1])?;
            let tf = args.get(2).map_or(Ok(Timeframe::D1), |v| parse_timeframe(v))?;
            let limit = args
                .get(3)
                .map_or(Ok(50_usize), |v| v.parse::<usize>().map_err(|_| "invalid bar limit".to_string()))?;
            if !(1..=500).contains(&limit) {
                return Err("history limit must be 1–500".into());
            }
            let bars = provider.history(&symbol, tf, None, limit).await.map_err(|e| format!("{e:#}"))?;
            Ok(json!({"provider":provider.name(),"symbol":symbol,"timeframe":tf.label(),"bars":bars}))
        }
        "search" if (2..=15).contains(&args.len()) => {
            let query = args[1..].join(" ");
            if !valid_expression(&query) {
                return Err("search expression too long or contains controls".into());
            }
            let hits = provider.search(&query).await.map_err(|e| format!("{e:#}"))?;
            Ok(
                json!({"provider":provider.name(),"query":query,"results":hits.into_iter().take(40).collect::<Vec<_>>()}),
            )
        }
        "book" if (2..=3).contains(&args.len()) => {
            let symbol = checked_symbol(&args[1])?;
            let depth =
                args.get(2).map_or(Ok(20_usize), |v| v.parse::<usize>().map_err(|_| "invalid depth".to_string()))?;
            if !(1..=50).contains(&depth) {
                return Err("depth must be 1–50".into());
            }
            let book = provider.order_book(&symbol, depth).await.map_err(|e| format!("{e:#}"))?;
            Ok(json!({"provider":provider.name(),"book":book}))
        }
        _ => Err(format!("unsupported CLI command: {head}; use --help")),
    }
}

fn app_call(command: Command) -> Result<Value, String> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path).map_err(|_| {
        format!(
            "Terminal-rs app not running or CLI socket unavailable at {}; open /Applications/Terminal-rs.app",
            path.display()
        )
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(7))).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(4))).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec(&command).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err("command too large".into());
    }
    stream.write_all(&bytes).map_err(|e| e.to_string())?;
    stream.shutdown(std::net::Shutdown::Write).map_err(|e| e.to_string())?;
    let mut output = Vec::new();
    stream
        .take((MAX_MESSAGE_BYTES * 16) as u64)
        .read_to_end(&mut output)
        .map_err(|e| format!("Terminal app did not reply: {e}"))?;
    let reply: Reply = serde_json::from_slice(&output).map_err(|e| format!("invalid app response: {e}"))?;
    if reply.ok {
        Ok(reply.data.unwrap_or(Value::Null))
    } else {
        Err(reply.error.unwrap_or_else(|| "app request rejected".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_every_mcp_timeframe_and_human_aliases() {
        for (text, expected) in [
            ("M1", Timeframe::M1),
            ("M5", Timeframe::M5),
            ("M15", Timeframe::M15),
            ("M30", Timeframe::M30),
            ("H1", Timeframe::H1),
            ("H4", Timeframe::H4),
            ("D1", Timeframe::D1),
            ("W1", Timeframe::W1),
            ("MN1", Timeframe::MN1),
            ("1D", Timeframe::D1),
            ("5m", Timeframe::M5),
        ] {
            assert_eq!(parse_timeframe(text).unwrap(), expected, "{text}");
        }
        assert!(parse_timeframe("INVALID").is_err());
    }
    #[test]
    fn rejects_execution_and_shell_commands() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        for args in [
            vec!["buy".into(), "AAPL".into()],
            vec!["app".into(), "order".into(), "BUY AAPL".into()],
            vec!["app".into(), "exec".into(), "rm -rf".into()],
        ] {
            assert!(rt.block_on(execute(&args)).is_err());
        }
    }
}
