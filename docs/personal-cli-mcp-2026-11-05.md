# Terminal-rs 개인용 CLI / MCP 연동

## 목적

Terminal-rs는 **시장 조회 및 분석 전용 앱**이며 실제 매매, 주문 취소,
거래 전자서명, 지갑 조작, 임의 셸 명령이나 소스코드 편집 기능을
CLI/MCP로 제공하지 않는다.

두 기능을 분리했다.

1. **독립 CLI** (`terminal-cli`): 앱이 꺼져 있어도 Terminal-rs가 쓰는
   `tr-data` 공급자 엔진으로 시세·일봉·종목 검색·공개 호가를 조회한다.
2. **실행 중 앱 제어** (`terminal-cli app ...`): 사용자에게만 접근
   가능한 Unix 도메인 소켓으로 앱 내부의 관심종목, 스크리닝, 알림,
   차트 및 캐시된 시세를 조회하거나 변경한다. UI 스레드가 직접 처리하고,
   결과를 JSON으로 반환한다.

MCP는 위 CLI에 단순 JSON 인자를 전달하는 **로컬 stdio Add-on**으로,
공개 HTTP 서버나 네트워크 수신 포트가 없다. Shellby Core 파일을 변경하지
않고 MCP Loader를 통해 연결한다.

## 빌드 및 개인용 설치

```sh
cd /Users/sim/Dev/terminal-rs
bash scripts/install-personal-macos.sh
```

이 명령은 `cargo build --release -p terminal -p terminal-cli`를 수행한다.
설치되는 앱은 `/Applications/Terminal-rs.app`, CLI는
`/Users/sim/Dev/terminal-rs/target/release/terminal-cli`이다.

개인 컴퓨터에서만 로컬 설치하며 자동 업데이트, 공증, 공개 배포는 없다.
기존 실행 중인 앱을 덮어쓰지 않도록 먼저 종료 여부를 확인하고,
설치에 실패하면 종전 앱을 복원한다. `TR_INSTALL_DIR`로 설치 폴더를
지정할 수 있다. Finder로 실행한 앱은 Apple Stocks 캐시 연동을
활성화한다.

## CLI 예시

```sh
CLI=/Users/sim/Dev/terminal-rs/target/release/terminal-cli

# 앱을 시작하지 않고 시장 조회
TR_APPLE_STOCKS_CACHE=1 "$CLI" quote AAPL 005930.KS
"$CLI" history "AAPL" D1 50
"$CLI" search "Samsung"
"$CLI" book "BTCUSD Curncy" 20

# 아래 항목은 Terminal-rs.app이 실행 중일 때만 동작
"$CLI" doctor
"$CLI" app status
"$CLI" app watchlist
"$CLI" app add "AAPL"
"$CLI" app remove "AAPL"
"$CLI" app screen "VOL 2"
"$CLI" app alert "AAPL ABOVE 200"
"$CLI" app technical-alert "AAPL RSI ABOVE 70"
"$CLI" app chart "AAPL"
"$CLI" app quotes "AAPL"
```

명령은 JSON 형태의 `{"ok":true,"data":...}` 또는
`{"ok":false,"error":...}`를 출력한다. 실패 시 종료 코드가 0이 아니다.
관심종목/스크리닝/알림은 앱 설정 저장 로직을 공유한다.

## sSb MCP Loader

개인 컴퓨터의 **Git 추적 제외** 설정:

`/Users/sim/Dev/sSb/.shellby/addons.local.json`

기존 REA, PdfCraft Add-on 항목은 보존하고 다음 항목을 추가한다.

```json
{
  "id": "terminal_rs",
  "enabled": true,
  "manifest": "/Users/sim/Dev/terminal-rs/mcp/addon.json"
}
```

프로젝트가 소유하는 파일:

- `mcp/addon.json` — Loader가 검증하는 MCP 매니페스트
- `mcp/server.mjs` — 표준 입력·출력 MCP 서버(Node 내장 모듈만 사용)
- `mcp/schemas/*.json` — 등록 도구의 JSON Schema
- `crates/control` — 공유 IPC 요청/응답 명세
- `crates/terminal-cli` — 독립 CLI 실행 파일
- `app/src/ipc.rs`, `app/src/app/ipc_commands.rs` — 앱과 통신

등록 도구 10개:

| MCP 도구 | 기능 | 앱 실행 필요 |
|---|---|---|
| `terminal_status` | 실행 상태, 데이터 소스, 읽기 전용 상태 | 아니오 |
| `terminal_quote` | 현재 시세 및 출처 | 아니오 |
| `terminal_history` | 기간별 OHLCV (최대 250개) | 아니오 |
| `terminal_search` | 종목 검색 | 아니오 |
| `terminal_book` | 공개 호가창 조회 | 아니오 |
| `terminal_watchlist` | 관심종목 조회·추가·제거 | 예 |
| `terminal_screen` | 관심종목 스크리너 설정 | 예 |
| `terminal_alert` | 가격·기술지표 로컬 알림 | 예 |
| `terminal_open_chart` | 앱 차트 종목 전환 | 예 |
| `terminal_app_quotes` | 앱에 이미 적재된 시세 조회 | 예 |

**sSb MCP Loader는 시작 시 매니페스트를 읽기 때문에 설정을 바꾼 뒤
sSb를 다시 시작해야 현재 대화에서도 새 도구가 보일 수 있다.**
실행 중인 sSb 프로세스를 자동으로 종료하거나 재시작하지 않는다.

## 보안·제한

- 소켓은 `~/Library/Application Support/terminal/terminal-control.sock`.
  파일 권한 `0600`, 로컬 Unix 도메인 소켓이며 TCP 포트는 없다.
- 한 번에 받은 명령은 제한된 JSON 프로토콜만 허용한다.
  `BUY`, `SELL`, `TRADE`, 주문 취소, 쉘 실행, 파일 편집은 없음.
- 앱이 종료되면 CLI의 `app` 명령은 실패한다. 시세 조회는 독립 가능.
- 앱에서 사용 중인 데이터 소스와 독립 CLI의 환경 변수는 다를 수 있다.
  MCP manifest에서는 `TR_APPLE_STOCKS_CACHE=1`을 기본 지정하되
  `TR_APPLE_STOCKS_LAUNCH=0`으로 주식 앱 자동 실행을 명시적으로 막는다.
- 동일한 데이터 폴더를 쓰는 앱 두 개가 동시에 실행될 때는
  소켓을 선점한 첫 번째 앱만 CLI 제어를 받는다.
- MCP는 **실행 중인 앱의 분석 작업을 조작**한다.
  Rust 소스 수정, 테스트, Git commit/push는 기존 sSb의
  `shell_run` 등의 개발 도구에서 수행해야 한다.

## 검증

- `cargo test --workspace`: **387 통과, 실패 0, 제외 6**.
- `cargo clippy --workspace --all-targets -- -D warnings` 통과.
- 임시 `TR_DATA_DIR`에서 GUI와 CLI를 동시에 실행해 상태·관심종목·
  차트·스크리너·알림 호출 실험
- 소켓 `0600` 검증
- sSb의 `loadAddonManifest`, `AddonManager.callTool`을 통해
  도구 스키마와 실수신 MCP 응답 검증
- 실제 설치된 Release `/Applications/Terminal-rs.app`와 MCP를
  임시 데이터 폴더에서 함께 실행하여 10개 도구 등록, 앱 상태,
  관심종목 수정, 스크리닝·알림·차트 전환, 시세 조회 확인.
  최종 GUI smoke: **14초 843프레임, 정상 종료**, Metal GPU.
- `terminal_history`의 `D1` 등 MCP 타임프레임 표기와
  시장 데이터 검색/공개 호가 조회도 정식 Loader를 통해 검증.
