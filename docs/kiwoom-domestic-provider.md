# Terminal-rs — Korean market data via Kiwoom

## What changed

The **existing GUI, pane layout, chart engine, and watchlist/alert logic remain
unchanged**. Only the native `tr-data::markets::Markets` router gains a
read-only domestic data provider.

| Asset | Provider | Route |
|---|---|---|
| KOSPI | Kiwoom REST / WebSocket | `KOSPI Index` → 001 |
| KOSDAQ | Kiwoom REST / WebSocket | `KOSDAQ Index` → 101 |
| KOSPI 200 | Kiwoom REST / WebSocket | `KOSPI200 Index` → 201 |
| Listed Korean equities, ETFs, ETNs | Kiwoom REST / WebSocket | `005930.KS`, `035720.KQ`, six-digit codes |
| US and other overseas securities | **Unchanged** | Yahoo/Alpaca/Apple cached quotes |
| Foreign exchange, foreign indices, futures | **Unchanged** | Yahoo etc. |
| Cryptocurrency | **Unchanged** | Binance/Coinbase/Terminal server |
| US Treasury yields | **Unchanged** | US Treasury |

Stored legacy keys such as `005930.KS US Equity` remain loadable; the
suffix and numeric code select Kiwoom at the data layer. `^KS11` and
`^KQ11` are accepted as alternate Korean index tickers.

## Authentication

Uses the already configured **official Kiwoom CLI** installed at
`~/.local/bin/kiwoomcli`. CLI tokens and App Keys remain in the existing
operating-system credential store. The Terminal-rs Rust process only invokes
allowlisted **read-only** `kiwoomcli domestic` commands; it never reads
credentials, calls an orders API, receives account data, or writes secrets.

Settings (optional, local environment only):

- `TR_KIWOOM_ENABLED=0`: disable Kiwoom provider entirely, restoring
  legacy market data routing for Korean instruments.
- `TR_KIWOOM_CLI=/absolute/path/to/kiwoomcli`: override CLI location.
- `TR_KIWOOM_PROFILE=<existing-profile-alias>`: select an already
  registered account's read-only market-data context; default is the
  profile selected by Kiwoom CLI.
- `TR_PROVIDER=sample`: original synthetic/offline demo mode, unchanged.

Do **not** add App Keys, secrets or tokens to environment variables or Git.
The production profile can be used for **read-only** market data without
creating or modifying orders.

## API mappings

| Purpose | Official CLI / Kiwoom TR |
|---|---|
| Individual stock/ETF quote | `domestic quotes price` / ka10007 |
| KOSPI/KOSDAQ/KOSPI200 quote | `domestic sectors price` / ka20001 |
| Stock 1/5/15/30/60-minute candles | `domestic candles stock-minute` / ka10080 |
| Stock daily / weekly / monthly | `domestic candles daily/stock-weekly/stock-monthly` / ka10081/82/83 |
| Index 1/5/15/30/60-minute candles | `domestic candles sector-minute` / ka20005 |
| Index daily / weekly / monthly | `domestic candles sector-daily/weekly/monthly` / ka20006 etc. |
| Korean stock/ETF names | `domestic stocks info-list` / ka10099 |
| Domestic order book | `domestic orderbooks list` / ka10004 |
| Stock trading feed | `domestic streams trades --watch` / WebSocket 0B |
| Index trading feed | `domestic streams sector-index --watch` / WebSocket 0J |

History normalizes Kiwoom's descending time order to ascending OHLCV. Index
**historical candles** encode price multiplied by 100, while index quotes
already use decimal prices. A leading minus sign in Kiwoom stock prices
indicates downward movement, **not** a negative asset price. Previous close
and the Kiwoom observation timestamp are used rather than the Yahoo daily
candle's 09:00 timestamp. H4 resamples 60-minute candles.

The market-data source field reads `Kiwoom REST` for Korean quotes.
CLI requests are spaced by 280ms within each Terminal-rs process to
stay below Kiwoom's 5 requests/sec REST quota. Other independent
processes using the same key can consume some of the quota; avoid
large concurrent batch jobs.

### Real-time behavior

- **Watchlist and overview panels** query fresh quotes on the existing
  20-second interval. There is no GUI layout change.
- **Open domestic charts** use the official CLI's authenticated WebSocket
  stream. The chart connection is cancelled when the receiver is dropped
  and reconnects on disconnect. Each active domestic chart currently has
  its own WebSocket CLI process.
- **After exchange close**, a WebSocket feed may be silent. The last genuine
  REST quote retains its provider timestamp; no synthetic tick is created.
- **News and unsupported domestic derivatives** are not claimed to be
  Kiwoom-supported: current news integration remains separate. Order
  transmission, account balances, and trade execution are not exposed.

## Existing CLI and sSb MCP integration

The pre-existing `terminal-cli` binary and all ten
`terminal_rs` MCP Loader tools are preserved. Because both use the
same `tr_data::from_env()` router, they automatically get Kiwoom
for Korean symbols, without an extra MCP add-on.

### Screenshot watchlist groups (2026-11-05)

The user's 14 Kiwoom favorites screenshots were transcribed into
`docs/kiwoom-screenshot-watchlists-source-2026-11-05.json`. Symbol
codes/exchanges were matched against the live official Kiwoom stock
catalog in `docs/kiwoom-screenshot-watchlists-resolved-2026-11-05.json`:
149 source rows, 147 entries after removing two same-group duplicates,
and 140 unique Korean instruments. A name that appeared in multiple
groups remains in each group.

To import into a running local GUI (read-only market-data use):

```sh
target/release/terminal-cli app groups-import \
  docs/kiwoom-screenshot-watchlists-resolved-2026-11-05.json
target/release/terminal-cli app watchlist
```

The original personal Watchlist is preserved as `symbols` in app output
and as the selectable **기존 관심종목** view. The 14 groups are stored
separately with their own names, order and collapse state and are
accessible in the sidebar and the full Watchlist pane's group picker.
Groups start collapsed. Only expanded/sidebar-visible groups and the
selected Watchlist-pane group are periodically polled; requests rotate
in batches of at most 40 symbols, with active price-alert instruments
prioritized, to respect Kiwoom REST rate limits. Display names come
from the existing Kiwoom catalog; source market keys are not rewritten.

Re-importing this JSON replaces only the imported groups, not the old
personal Watchlist instruments already present at import time, layout,
alerts or preferences. The
GUI saves imported groups in its existing local `settings.json` file
and restores them after restart. The import IPC is allowlisted,
validated and never sends trade or brokerage instructions.

```sh
cd /Users/sim/Dev/terminal-rs
target/release/terminal-cli quote KOSPI KOSDAQ 005930.KS AAPL
target/release/terminal-cli history KOSPI M1 30
target/release/terminal-cli history 005930.KS D1 40
target/release/terminal-cli search 삼성
target/release/terminal-cli book 005930.KS 10

# Live WebSocket diagnostic; outside market hours it may report no trades:
cargo run -p tr-data --example kiwoom_realtime_probe -- "KOSPI Index"
```

`terminal_quote`, `terminal_history`, `terminal_book`, and
`terminal_search` MCP calls have the same routing. App-control tools
continue to require the running Terminal-rs GUI.

## Deployment and testing

Install the new Release binary using the existing private installer:

```sh
bash scripts/install-personal-macos.sh
```

After installation, restart any running older instance of Terminal-rs to
activate the new router. Existing user layouts, watchlists and preferences
are not migrated or cleared.

Real-data validation (2026-11-05, existing authenticated CLI):

- Kiwoom REST: 005930 current price; KOSPI/KOSDAQ/KOSPI200 quotes;
  005930 and KOSPI history (minute, daily, weekly, monthly);
  005930 public order book and Korean-name search.
- Overseas control: AAPL returned from the original Yahoo provider.
- Kiwoom WebSocket 0J: native Rust subscriber received a KOSPI index
  bar. Stock 0B may be silent outside exchange hours.
- KOSPI after-close response includes an invalid time sentinel (`888888`);
  the quote parser skips it and uses the next genuine exchange observation
  (`15:32:20` in the captured sample), rather than marking 09:00 or
  assuming the HTTP request time was a trade time.
- Without changing the GUI layout, Korean quotes now show
  `Obs HH:MM:SS KST` beside the market name in the world-markets overview,
  and in the chart's existing bottom status row when space permits.
  Hovering a quote still shows its complete provider/timestamp provenance.
  The daily-candle time axis remains at 09:00 (bar opening), separate
  from the actual exchange quote observation.
- Korean-market **intraday chart X-axis ticks and crosshair timestamps**
  now use exchange-local **KST (UTC+09:00)** instead of the former raw UTC
  formatter. The plot's right-hand time-axis area shows `KST`, and the
  crosshair timestamp includes the timezone suffix. Actual bar Unix times
  and candle OHLCV data are unchanged. This corrects the nine-hour chart
  display offset (e.g., 15:30 KST was previously drawn as 06:30).
  Overseas charts continue using their existing timestamp format.
- Korean stocks, ETFs and Korean indices use local market chart colors:
  **red for rising / blue for falling** candlesticks, OHLC bars and volume,
  with matching chart price labels and direction legend (both dark and
  light themes). Overseas market charts retain their existing green/red
  or user-selected theme colors; only Korean **charts** are affected.
- Domestic symbols are displayed as Korean **company/index names** in the
  Watchlist, watchlist screen, chart headers/legends, and chart tabs
  (`005930.KS` → `삼성전자`, `000660.KS` → `SK하이닉스`,
  `KOSPI` → `코스피`, `KOSDAQ` → `코스닥`). The saved symbol keys,
  price queries, alerts, CLI, and MCP APIs **remain unchanged**. Users can
  find the technical symbol in the sidebar hover text. Other domestic
  stock names are resolved asynchronously through the existing Kiwoom
  `stocks info-list` catalog and displayed once available; when a name
  cannot be resolved, the original symbol remains visible. On macOS, the
  app registers the locally installed Apple Gothic typeface as an egui font
  fallback for both chart/Watchlist text families so Hangul names render
  correctly; no system font is copied or packaged with the app.
- Active sSb MCP Loader: `terminal_quote` routed KOSPI/KOSDAQ/005930
  through Kiwoom, whereas AAPL remained on the existing Apple Stocks/Yahoo
  foreign-provider path; `terminal_history`, `terminal_book`, and
  `terminal_search` all returned real Kiwoom results.
- New signed Release application installed to
  `/Applications/Terminal-rs.app`; GUI/IPC end-to-end test used a
  temporary application data directory and confirmed both KOSPI and
  005930 price, source, and observation timestamps in the live app's
  watchlist cache. macOS Metal smoke: 30 seconds, 1,787 frames, clean exit.
- `cargo test --workspace`: 392 tests passed, six ignored;
  `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- Unit tests: Korean vs foreign routing; signed price decoding;
  Korean observation timestamps; index OHLC ×100 normalization;
  realtime event decoding.

No API key or token is committed. If Kiwoom is disabled/not installed,
legacy provider routing is preserved; when installed and authenticated,
domestic API errors do not silently invent prices or order anything.
