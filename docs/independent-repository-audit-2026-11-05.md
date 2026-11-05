# terminal-rs 독립 관리 및 원본 커밋 점검 (2026-11-05)

## 원격 저장소 관계

- 개인 저장소: `https://github.com/cantusprote/terminal-rs.git`
- 로컬 브랜치: `master`, 추적: `origin/master`
- 원본 저장소의 `upstream` Git remote 제거 완료
- 원본 `michaelshimeles/terminal`의 현재 `master` 최신 SHA:
  `a12b5d14e4c25c2c4f1ce6a8c95e2f8fb30011cf`
- 이 커밋은 이미 개인 저장소의 `3f0cede` 조상이다.
  `git log HEAD..a12b5d1`에 해당하는 **새 커밋이 없었다**.
  원본에서 새 코드를 가져오거나 병합할 필요가 없었다.

## 원본 최신 개선사항 비교

| 원본 커밋 | 개선사항 | 현재 코드 |
|---|---|---|
| `a12b5d1` | Turnkey 내장 지갑, Hyperliquid testnet | 이미 포함 |
| `08f5464` | Hyperliquid testnet 거래 기능 | 이미 포함 |
| `5bcf3a5` | 주문장 갱신 주기 단축 (~0.5초) | 이미 포함 |
| `0690de8` | 게이트웨이 속도·접속 보호 | 이미 포함 |
| `1b7ab34` | HIP-3 perpetual market | 이미 포함 |
| `94bb329` | 데이터 공급자 변경 후 과거 차트 재로딩 | 이미 포함 |

원본 이후 개인 저장소의 Apple Stocks 캐시 및 신규 종목 조회 기능은
`3f0cede` 커밋에 있다. 이를 유지하면서 독립 릴리스 경로를 정리했다.

## 개인 저장소용 수정

1. `crates/macos/src/updater.rs`: 릴리스 URL을 개인 저장소로 변경
2. `scripts/bundle-macos.sh`: 원본의 Sparkle 업데이트 피드 및 공개 키 제거.
   기본 앱은 자동 업데이트를 사용하지 않으며, 독립적으로 생성한
   `TR_SPARKLE_PUBLIC_KEY`가 있을 때만 Sparkle 프레임워크와 개인 피드를 포함
3. `scripts/release-macos.sh`: 릴리스 주소, 키 계정, 서명 검증을 개인 저장소용으로 변경.
   독립 서명 키 없이는 서명된 업데이트 피드를 배포하지 않음
4. `deploy/freestyle/vm-setup.sh`: 서버 설치 시 원본이 아닌 개인 소스 사용
5. `site/src/lib/site.ts`, `site/README.md`: 저장소 및 다운로드 링크를 개인 주소로 변경
6. `.github/workflows/ci.yml`, `.github/workflows/deploy-site.yml`:
   자동 빌드/배포를 비활성화하고 필요할 때 `workflow_dispatch`로만 실행
7. `.gitignore`: Finder의 `.DS_Store` 제외

## 검증

- `cargo test --workspace`: **501 passed, 0 failed, 8 ignored**
- `bash -n`으로 릴리스/번들/서버 설치 스크립트 문법 확인
- `scripts/bundle-macos.sh target/debug/terminal`: 독립형 `Terminal.app` 생성 성공
- 이 기본 번들에는 `SUFeedURL`, 원본 Sparkle 공개 키, Sparkle 프레임워크 없음
- Rust formatting, Git diff whitespace, compiler 및 lint 검증은 별도 확인

## 추가로 분리할 수 없는 부분

**GitHub Fork network:** Git remote를 끊는 것으로 GitHub의 `fork=true` 관계가
삭제되지는 않는다. GitHub UI 저장소 `Settings → General → Danger Zone →
Leave fork network`에서 별도 확인이 필요하다. 해당 기능은 영구적이며
issues, pull requests, stars 등 메타데이터를 잃을 수 있다. 저장소 삭제/재생성은
실행하지 않았다.

**원본 코드의 재배포 권한:** 현재 원본 저장소에는 명시적인 LICENSE 파일이나
GitHub license 메타데이터가 없다. 공개 코드/기존 Fork라는 사실만으로
독립적인 재배포 권한이 자동 부여되지는 않는다. GitHub Fork network를
실제 분리하기 전에 원저작권자의 사용·재배포 허가 여부를 확인할 필요가 있다.

**외부 서비스:** 지갑 로그인 URL `terminal.rasmic.xyz/wallet`, 사이트 기본
주소 및 Turnkey·Hyperliquid 연결은 원래 서비스에 의존한다. 이것은 GitHub
저장소 원격과 다른 문제이다. 완전한 제품·서비스 독립화를 위해서는 별도
Turnkey 구성, 웹 사이트 및 서버 배포가 필요하다. 이번 변경은 이 서비스들을
임의로 전환하거나 지갑 동작을 깨뜨리지 않았다.
