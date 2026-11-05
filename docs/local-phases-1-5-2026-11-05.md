# 개인용 Terminal-rs Phase 1–5 개선 및 검증 보고

작성일: 2026-11-05 (Asia/Seoul)

**목표:** Mac Studio와 MacBook에서 개인적으로 실행하는 **조회·분석 전용**
터미널 개선. 공개 배포·자동 업데이트·상용 거래 기능은 범위에서 제외.
개인 GitHub `cantusprote/terminal-rs`를 코드 원본으로 유지한다.

## Phase 1 — 읽기 전용 안정화

- `tr-hl`, `tr-turnkey`, SnapTrade 주문·취소·서명 코드는 이전 단계에서
  소스/의존성에서 제거됨. 이번 단계에서 워크스페이스 및 호출 경로 재감사.
- `BUY`, `SELL`, `TRADE`, `MODE LIVE` 등은 계속 파서 단계에서 차단.
- 로컬 설정의 `trading` 필드가 오래된 파일에 있더라도 활성화되지 않음.
- 프로세스 안에서 주문 실행/취소/거래 서명을 수행할 수 없음.
- 증권사의 **기존 주문내역 조회** 및 정부의 **정치인 매매공시 조회**는
  실제 매매 기능이 아니므로 유지.

## Phase 2 — 시세 데이터 정합성 및 Apple Stocks

- 기존 구현에는 Apple Stocks `stocks://?symbol=` 검색으로 누락 시세를
  불러오는 기능이 있었으나, 2026-11-05 실사용 점검에서 Stocks 앱이
  `⌘Q` 종료 후에도 다시 열리는 문제가 확인되어 **자동 실행 기본
  비활성화**. 현재는 로컬 캐시를 읽고 없으면 Yahoo로 대체한다.
- `TR_APPLE_STOCKS_LAUNCH=1`은 사용자가 특별히 허용했을 때만
  deep link로 Stocks 앱을 실행한다. Finder 앱과 MCP에는 기본 비활성화.
- 한 요청에서 최대 3종목 순차 조회 (중복/빈번한 탐색 방지); 동시에 여러
  작업자가 Stocks를 탐색하지 않도록 교차 요청 탐색을 직렬화.
- Apple SQLite 캐시의 `updatedAt`뿐 아니라 실제 시세 관측 시각인
  `dateLastRefreshed`도 검사. 갱신 시각만 최근인 오래된 시장 관측은 거부.
- Apple이 제공하는 `priceSource`의 지연 표시에 따라
  `Apple Stocks (delayed)` 표기. 종목별 `Quote.source` 필드 신설,
  종전 데이터가 `source` 없이 직렬화됐어도 역직렬화 가능.
- Apple Stocks는 주식·ETF·지수 우선, FX·원자재 등은 적절한 기존 데이터
  소스에서 조회. 누락/만료 시 Yahoo Finance로 대체.
- 시장 데이터 API가 실패했다고 가짜 가격으로 채우지 않음. 데모 목적으로
  `TR_PROVIDER=sample` 또는 `TR_SAMPLE_FALLBACK=1`을 지정한 경우에만
  합성 데이터를 사용하도록 제한.
- 관심종목/세계시장 행에 **실제 데이터 공급자·관측 시각·경과 시간** 툴팁 추가.
  최종 앱은 시장 휴장·지연 데이터가 실시간 데이터처럼 보이지 않도록 주의.
- 실제 Mac Studio에서 `005930.KS` 조회 시 지연된 Apple Stocks 시세가
  반환되는 현장 검증 수행. 가격은 시간에 따라 변하므로 보고서에 고정하지 않음.

## Phase 3 — 차트/지표 입력 정합성

- 들어오는 OHLCV 데이터에 대해 finite 값, 고가·저가 범위, 비음수 거래량을
  검증. 시각 역순과 중복 타임스탬프 정리.
- 과거 캐시, 네트워크, 추가 과거 이력, 비교 차트, 실시간 캔들 모두
  동일한 규칙 적용. 잘못된 캔들이 GPU 버퍼에 들어가는 것을 방지.
- 음수 채권 수익률 같은 실제 가능한 값은 잘못된 가격으로 단정하지 않음.
- 새 요청에서 빠진 과거 시세를 현재값으로 계속 표시하지 않고 제거하며,
  늦게 도착한 더 오래된 관측이 최신 관측을 덮어쓰지 않음.
- 테스트용 가짜 캔들에 고가보다 높은 종가가 있던 기존 테스트 픽스처도
  수정하고, 데이터 정규화 회귀 테스트 추가.
- 기존 지표 엔진/비교차트/렌더링 테스트도 재검증.

## Phase 4 — 로컬 스크리너 및 가격 알림

- watchlist 전용 로컬 필터: 상승률·하락률·절대 등락률·시각 최신성,
  표시 순서(원래 순서, 상승, 하락, 종목명). 일봉(최대 45개)으로
  **SMA20, RSI14(Wilder 평활), 최근 거래량/직전 20거래일 평균 거래량**
  계산도 추가. 기술적 조건의 데이터가 부족하면 종목을 제외.
- 명령 예: `SCAN UP 2`, `SCAN DOWN 2`, `SCAN MOVE 3`,
  `SCAN FRESH 10`, `SCAN SORT GAINERS`, `SCAN ALL`,
  `SCAN VOL 2`, `SCAN SMA ABOVE/BELOW`, `SCAN RSI HIGH/LOW`.
- 가격 돌파 알림: `PRICEALERT AAPL ABOVE 210`,
  `PRICEALERT 005930.KS BELOW 70000`, `PRICEALERT LIST`,
  `PRICEALERT CLEAR`. 관심종목 우클릭 시 현 시세 ±2% 빠른 추가 지원.
- 첫 시세를 기준값으로만 취급하고, **다음의 새로운 관측**이 기준선을
  넘어야 한 번만 알림. 30분 넘게 오래된 관측, 합성 샘플 또는
  공급자 미상 시세는 알림을 발생시키지 않음.
- 스크리너 설정·가격 알림 목록은 기존 `settings.json`에 저장/복원.
  기술적 지표 알림도 `TECHALERT AAPL VOL 2`,
  `TECHALERT AAPL RSI ABOVE 70`, `TECHALERT AAPL SMA BELOW`로
  별도 등록·저장·제거 가능. 5분 간격으로 관심종목/알림 대상의 일봉을
  갱신하며 동일한 OHLCV 관측으로 알림을 반복하지 않음.
  최신 거래량은 **진행 중인 일봉의 누적 거래량**일 수 있어 최종 거래량으로
  해석하면 안 됨. 관측 범위는 회당 최대 40개 종목.
  앱이 실행 중이고 네트워크 관측이 이루어져야 알림을 받을 수 있으며
  외부 알림 서버나 주문 실행은 없음.

## Phase 5 — macOS UI와 개인 사용 워크플로

- 관심종목 패널 폭이 좁을 때 일부 중복 컬럼 숨김.
- 저장된 관심종목 목록이 비어 있어도 재시작 시 기본 목록으로 덮어쓰지
  않으며, 중복 종목도 정리. 이전 설정 파일은 계속 읽을 수 있음.
- 전역 데이터 공급자 라벨이 실시간 보장으로 오인되지 않도록 수정.
- `UPDATE` 명령은 GitHub 공개 릴리스 화면 대신
  `git pull --ff-only origin master` 및 로컬 재빌드 안내.
- 앱에서 Sparkle 자동 업데이트를 실행하지 않음.
- `cargo build -p terminal` 후 `scripts/bundle-macos.sh target/debug/terminal`로
  로컬 `target/debug/Terminal.app` 생성. 별도 공개 배포 없음.

## 검증과 제한

- Rust 전체 워크스페이스 테스트, Clippy 전체 targets 경고 0,
  `cargo fmt`, `git diff --check`를 수행.
- 최종 검증: `cargo test --workspace` **383 통과, 0 실패, 6 제외**.
  `cargo clippy --workspace --all-targets -- -D warnings` 통과.
- Node 24를 사용한 Svelte 타입 검사·웹 빌드는 계속 통과하는지 확인.
- macOS Metal GPU 앱 생성 및 `TR_SMOKE_SECS=5` 자동 실행·종료 확인.
  최종 Mac Studio M1 Max: **5.0초 290프레임**, 정상 종료,
  UI CPU 약 2.63ms/프레임.
- 개인용 `Terminal.app` 번들 생성 성공, Info.plist 유효,
  현재 컴파일된 실행 파일과 번들 내 파일 일치,
  Sparkle 업데이트 프레임워크 불포함.
- 실제 시세는 공급자·시장별로 지연될 수 있음. Apple의 비공개 SQLite
  캐시 구조는 macOS 업데이트에서 변경될 수 있고, 새 종목 검색을 항상
  보장하지 않음.
- 로컬 알림은 OS 백그라운드 데몬이나 클라우드 감시가 아니며, Terminal을
  종료하면 작동하지 않음. 알림 발생 기록 자체는 다음 실행을 위해 보존.
- 정식 공개 릴리스, notarization, 배포 웹사이트/클라우드 배포는 미시행.
- 개인 컴퓨터의 OS 키체인/증권사 인증정보를 수정하지 않음.

## 로컬 실행

```sh
cd /Users/sim/Dev/terminal-rs
cargo build -p terminal
scripts/bundle-macos.sh target/debug/terminal
TR_APPLE_STOCKS_CACHE=1 ./target/debug/terminal
```

별도의 시세 분석 테스트는
`TR_APPLE_STOCKS_CACHE=1 cargo run -p tr-data --example apple_stocks_probe -- --route 005930.KS`.
