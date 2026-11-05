# Apple Stocks 8.5 — REA investigation log

Inspected on macOS 26.7 using `rea` 6.4.0. All investigations were
read-only and launched with the working directory set to this `mac_stock/`
folder. Apple binaries, cache payloads, identities, tokens, and user watchlists
are deliberately **not** copied here.

## Native application boundary

| REA operation | Target | Evidence ID |
|---|---|---|
| `inspect-macho` | `/System/Applications/Stocks.app/Contents/MacOS/Stocks` | `ev_e004d68a0a16d9271d34e42939fd7425b227e3649a9d2b2490bca95532073057` |
| `inspect-macho` | `/System/Library/PrivateFrameworks/StocksKit.framework/XPCServices/StocksKitService.xpc/Contents/MacOS/StocksKitService` | `ev_ee4f822debe6e8a7f788a9612a9dd040ee659ea91bc7f86724b47cc3d0074f0f` |
| `inspect-native-dispatch-metadata` | StocksKitService | `ev_bb5c3873c0a86dc61f41464d2dff85bcb481aeda515de664bfd4f7037e6049ce` |
| `inspect-plist` | `/System/Applications/Stocks.app` | `ev_84db0e445c7410ae44c0181976b16a8b67332b40816b650c80d874a9d900cf25` |

The native Stocks executable is arm64e/x86_64 universal and imports the private
`StocksCore` and `StocksKit` frameworks. `StocksKitService` also imports both.
REA found the service's Swift `Assembly` type, but this observation does **not**
identify or certify a stable data-request interface. The framework implementation
is part of the system's shared cache, so the observed entry-point boundaries do
not prove the underlying network URL or authorization contract.

## Local data boundary

| REA operation | Cache | Evidence ID |
|---|---|---|
| `inspect-sqlite-database` | `group.com.apple.stocks/Library/Caches/shared-database` | `ev_a631f7c39c7dae4a5193b226a461d186579a4ceb58f17fe92a1e8763fe610b03` |
| `inspect-sqlite-database` | `group.com.apple.stocks/Library/Caches/sparkline-database` | `ev_4c8faf6401cbf1d4a1d985ea607920524b59320d3f2818af51dd24ba7b430976` |

At inspection, `shared-database` contained 336 quote records, with `quotes`
columns `(id TEXT, updatedAt DOUBLE, valueJson BLOB)`. Each decoded quote
JSON has wrapper `v` and properties such as `price`, `priceChange`,
`dateLastRefreshed`, `exchangeStatus`, and `priceSource`. The same database
also contains `sds_auth_tokens`. **The implementation never reads this table.**

The `sparkline-database` contained 31 `sparklines` records with columns
`(id TEXT, updatedAt DOUBLE, valueJson BLOB)`. `v.entries` contains reference
date, open, high, low, close, and volume. Observed consecutive entries were
predominantly 600 seconds (10 minutes) apart. No historical depth beyond the
cached sparkline should be assumed.

`updatedAt` is Unix epoch seconds. The per-quote `dateLastRefreshed` and
per-sparkline `referenceDate` fields are Apple reference seconds (Unix offset
`978307200`). Mixing the epochs would produce incorrect chart timestamps.

At one observed point, AAPL had `priceSource="nasdaqRealtime"` while `^KS11`
had `priceSource="genericDelayed"`. These labels are copied from Apple's
cache and are **not** a guarantee that every quoted instrument is real-time.

The quotes table also contained **58 `.KS` symbols and 29 `.KQ` symbols**.
The pre-existing terminal parser rejected exchange-qualified numeric tickers
such as `005930.KS`, and its Yahoo symbol mapper incorrectly rewrote `.KS`
to `-KS`. Both parsing and mapping were updated to retain Korean exchange
suffixes without interpreting standalone numbers as ticker commands.

## New-symbol refresh experiment (November 5, 2026)

REA `inspect-plist` confirms Stocks 8.5 has `CFBundleURLSchemes=[stocks]`
and `NSUserActivityTypes` including `OpenSymbolIntent`. Apple's published
Apple News Format documentation also describes `stocks://?symbol=AAPL` as a
supported link to a ticker in the Stocks app. We tested this route via the
macOS application launcher (`open -g`) and examined the read-only local SQLite
cache before and after each call:

| Test | Initial local quotes row | Observed result |
|---|---|---|
| ORCL via `open -g 'stocks://?symbol=ORCL'` | Absent | A new row appeared in about one second |
| ADBE via `Markets::quotes` | Absent | Terminal returned a quote and the Apple row appeared |
| `035900.KQ` via `Markets::quotes` | Absent | A new Korean KOSDAQ quote appeared and was returned |

The `-g` option left Firefox in the foreground during the test. Apple's
Stocks app may still navigate its own window to the requested symbol. No
private HTTP endpoint or authentication method was cloned or invoked directly.
Success for these tickers does not prove support for every ticker or market.

## Implementation decision

An Apple-private HTTP/XPC adapter would depend on an undocumented authorization
contract that has not been reconstructed. Instead, the implemented macOS-only
`AppleStocksCache` opens the registered Stocks URL on a cache miss and reads
the two observed caches with SQLite `READ_ONLY`, extracting only market prices
and recent OHLCV for available symbols. The adapter polls for up to four
seconds and falls back to Yahoo if missing, stale or unsupported. The normal
Yahoo/Alpaca router supplies longer historical bars.
No authentication material is used, no system app is changed, and no cache write
is performed. The adapter is opt-in via `TR_APPLE_STOCKS_CACHE=1`.

Probe: `TR_APPLE_STOCKS_CACHE=1 cargo run -p tr-data --example apple_stocks_probe -- --route ADBE`.

The cache remains opportunistic: Apple's app decides when and which symbols to
refresh despite successful deep-link delivery. To get guaranteed real-time streaming for arbitrary tickers, use an
officially supported market-data API instead.
