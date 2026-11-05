# Terminal-rs: market research and monitoring only (2026-11-05)

The personal `cantusprote/terminal-rs` project does not implement investment
order entry or signing. This includes *all* assets (stocks, crypto, futures,
HIP-3 perps, and ETFs), and also local paper-trading order simulation.

## Preserved market-analysis functions

- Apple Stocks cached quotes and deep-link cache refresh on macOS
- Yahoo/Alpaca equity and multi-asset data, Binance/Coinbase crypto data
- Hyperliquid's **public market-data gateway** (mids, candles, order books, historical trades)
- Charts, indicators, technical analysis, news, watchlists, search, options data
- Congress trading-disclosure research
- **Read-only** SnapTrade brokerage portfolios, balances, positions,
  *historical order records* and activity; account connection management
- Offline sample *quotes and portfolio display* (no simulated trade entry)

## Removed execution capabilities

- `crates/hl` (all Hyperliquid order signing, leverage, agent, placement,
  cancellation and authorization code) and `crates/turnkey` (wallet client)
- App's `hl_worker`, `wallet_worker`, order ticket, trading UI and account-mode
  module, trade confirmation dialogs, and related views
- SnapTrade `tr_broker::trading` order impact, place and cancel API
- Demo order execution and `tr_broker::demo` paper-trading engine
- Embedded Turnkey wallet web route, session handoff and JavaScript dependencies
- Old trading-specific screenshots and design document
- Trade/cancel widgets in chart, sidebar, watchlist, positions and orders
- `BUY`, `SELL`, `TRADE`, `KILL`, `MODE LIVE`, `DEMO`
  and related command entries. Parser rejects them; legacy Buy/Sell shortcut
  entries are inert and absent from the current hotkey catalogue.
- Previously saved `prefs.json.trading` is ignored on load, not reenabled.
- CI's obsolete `tr-hl` and `tr-turnkey` builds

## Safety and compatibility

The core brokerage client can **read** orders previously placed elsewhere,
but cannot submit or cancel any. Connecting, registering, or disconnecting a
SnapTrade user is still an account-management operation, *not* an exchange
order. Public market trades and Congress buys/sells are observational data.

**Existing user secrets in the OS Keychain were not accessed or deleted.**
Removing the software is not a transaction or an instruction to revoke an
existing third-party wallet, order, or brokerage permission. Those must be
managed separately at the provider.

## Validation

- `cargo test --workspace`: **366 passed, 6 ignored, 0 failed**
- `cargo clippy --workspace --all-targets -- -D warnings`: passes
- `cargo build -p terminal`: passes
- Web `npm run check`: 0 errors, 0 warnings
- Web `npm run build` with Node 24: passes
- Static search and `cargo tree` confirm no `tr-hl`, `tr-turnkey`,
  agent signing, exchange placement or order-cancel paths in active targets
- macOS app startup: Metal window/GPU initialization confirmed; initial
  background-only smoke run did not reach its timed close and was terminated
  explicitly, so full UI smoke success is **not** claimed for that run

The terminal **cannot** prevent a user from trading through a separate
brokerage application or an existing external wallet; it simply has no
execution functions of its own.
