# 1단계-5 검증 기록과 1단계 총괄 검증

이 문서는 1단계 전체(1-1 ~ 1-5)의 총괄 검증 문서다. 결과는 명령과 종료 코드로 적고, 실행하지 않은 것은 실행하지 않았다고 적는다. "검증 안 됨"은 완료를 막는 관문이 아니라, 사람이 모아서 확인하는 목록이다. 1단계 완료 기준은 맨 아래 체크리스트에 있다.

## 단위별 검증 문서

| 단위 | 범위 | 문서 |
|---|---|---|
| 1-1 | Tauri 뼈대, 잠금·다운타임·설정 이력 | `docs/stage-1-1-verification.md` |
| 1-2 | 캘린더 계획, 수정 히스토리, 공부·휴식 타이머 | `docs/stage-1-2-verification.md` |
| 1-3 | 앱 차단 코어, 건너뛰기·긴급 패스, 해제 절차 | `docs/stage-1-3-verification.md` |
| 1-4 | 브라우저 확장, 앱 연결, 차단 프리셋 | `docs/stage-1-4-verification.md` (통신 방식 결정: `docs/stage-1-4-transport-decision.md`) |
| 1-5 | 집중 측정, 포인트·레벨, 조기 신호, 대시보드 | 이 문서 (1-5 전용 문서는 따로 없음) |

## 환경

- (2026-11-07 추가) Windows 11 x64, rustc 1.99.1 네이티브 검증은 "Windows PC 검증 결과" 절 참고.
- macOS (darwin 23.4.0, arm64). Node와 pnpm은 `$HOME/.local/node/bin`을 PATH에 추가해 실행.
- 변경은 모두 미커밋(작업 트리).

## 1-5에서 만든 것

| 스토리 | 내용 |
|---|---|
| 5-S1 | 센서 `FocusProbe`(맨 앞 앱, 입력 유휴 초). 키 입력 내용과 창 제목은 수집하지 않는다. 공부 앱 목록(`study_apps`, M6). |
| 5-S2 | 집중 이벤트·분 단위 버킷(`focus_events`, `focus_minutes`, M7), 이탈·복귀·멍때림 계산, 집중 점수(인정 시간 / 공부 시간 x 100, D-25). |
| 5-S3 | 포인트·레벨(`point_ledger`, M8). `ref_key`로 멱등 지급, 레벨은 누적 획득 포인트 기준. 상수는 `points/rewards.rs` 한 곳. |
| 5-S4 | "방금 딴생각함" 전역 단축키(`CommandOrControl+Alt+Shift+D`)와 랜덤 체크인(15~40분). `focus_events.kind='self_report'` 사용. |
| 5-S5 | 대시보드. Rust 집계(`dashboard` 모듈) 3개 명령 `get_dashboard_today` / `get_dashboard_week` / `get_weakness`, 대시보드 화면(오늘이 처음 열리는 탭, 이번 주·약점은 선택 시 불러옴). 새 의존성 없음(막대·히트맵은 CSS). |

### 대시보드 정의 (5-S5)

- **계획 시간** = 실행(run)의 세트 수 x 공부 분 + 아직 실행이 없는 계획 발생분(계획이 생기기 전 발생분 제외). **실제** = 인정 시간(`focus_minutes.credited_ms`, 이탈·멍때림·휴식·다운타임 제외).
- 자정을 넘기는 실행은 계획된 날짜 하나에 귀속된다.
- **연속 달성일(D-29)**: 그날 계획의 80% 이상 인정. 계획 없는 날은 건너뛰고, 계획이 있었는데 80% 미만인 날에서 끊긴다. 오늘은 진행 중이면 끊지 않는다. 최대 366일 조회.
- **집중 점수 추세**: 끝난 공부 세트의 인정 시간 / (세트 길이 - 겹친 다운타임) x 100.
- **이번 주**: 월~일(로컬). 달성률은 요일별 인정 시간을 그날 계획으로 상한한 합 / 주 계획. 과목별 시간은 실행이 속한 계획의 과목 기준.
- **약점(일부)**: 최근 28일. 딴짓 앱 순위는 `block_events`의 앱 대상 시도(종료 요청, 시험 모드 적중)만 센다(사이트 호스트는 순위에 올리지 않는다). 히트맵은 이탈 이벤트와 차단 시도(앱·사이트)를 로컬 요일 x 시간으로 센다. 자주 밀리는 계획은 `plan_history`의 적용된 `postponed` 변경이 2회 이상인 계획. 꺼진 시간은 `downtime_records`의 절전·강제 종료를 따로 보이고, 계획 시간과 겹친 시간은 두 종류를 합집합으로 한 번만 센다. 긴급 패스·건너뛰기·중간 해제·중단된 실행 횟수도 함께 보인다.
- 성장 탭과 주간 LLM 인사이트는 3단계.

## 실행한 검증

5-S5 (에이전트가 범위 테스트로 실행):

| 항목 | 명령 | 결과 |
|---|---|---|
| 대시보드 집계(손계산 골든, 자정 넘김, 다운타임 겹침, 빈 DB, 진행 중 실행, 주 경계, 시간대 히트맵, 연속 달성일 표, 약점 전 항목, 로그 개인정보) | `cargo test dashboard` (src-tauri) | 18개 통과 (집계 17 + 와이어 계약 1) |
| 개인정보 스키마 가드 | `cargo test privacy_schema_guard` | 통과 (대시보드는 새 테이블·컬럼을 만들지 않음) |
| 와이어 계약(픽스처 `src/api/fixtures/dashboard-contract.json`) | `vitest run src/api/dashboard.test.ts src/pages/dashboardView.test.ts` | 통과 |
| 타입 체크, 린트 | `tsc --noEmit`, `eslint src` | 종료 0, 종료 0 |
| 대시보드 파일 clippy 경고 | `cargo clippy --all-targets` 출력에서 `dashboard` 검색 | 해당 경고 0건 |

전체 검증 결과.

| 항목 | 명령 | 결과 |
|---|---|---|
| 타입 체크 | `pnpm tsc` | 종료 0 |
| 린트 | `pnpm lint` | 종료 0 |
| 프런트 테스트 | `pnpm test` | 153개 통과 |
| 확장 빌드 | `pnpm build:extension` | 종료 0 |
| Rust 컴파일 | `cargo check --all-targets` (src-tauri) | 종료 0 |
| Rust 린트 | `cargo clippy --all-targets -- -D warnings` | 종료 0 |
| Rust 테스트 | `cargo test` | 367개 통과 |
| 개인정보 검사 ① 스키마 가드 | `cargo test privacy_schema_guard` | 1 통과 |
| 개인정보 검사 ③ 로그 인자 검색 | `tracing::` 호출 인자에 창 제목·URL·타이핑 원문이 없는지 `search`로 확인 | `tracing::` 인자에 창 제목·URL·타이핑 원문 없음 (tray.rs의 "title"은 메시지 문구) |
| 번들 | `pnpm tauri build` | 종료 0, .app/.dmg 생성 |
| Windows 컴파일 시도 | `cargo check --target x86_64-pc-windows-msvc` | 종료 101: 번들 SQLite C 빌드에서 `stdlib.h` 없음(호스트 헤더 부재). 검증 안 됨 |
| Windows 네이티브 검증 (2026-11-07, Windows 11 x64, rustc 1.99.1) | `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (src-tauri); `pnpm install --frozen-lockfile`, `pnpm tsc --noEmit`, `pnpm lint`, `pnpm test`, `pnpm build:web`, `pnpm build:extension`, `pnpm tauri build --bundles nsis` | 전부 종료 0. cargo test: lib 1047 + llm_wire 4 + remote_wire 7 + doc-tests 3 통과, 실패 0. vitest 72파일 511개 통과. NSIS 설치 파일 생성(설치·실행 미수행). 위 "Windows 컴파일 시도" 행(종료 101, 검증 안 됨)은 맥 호스트 교차 컴파일의 이전 결과이며 네이티브 Windows 결과로 대체됨 |

## 1단계 검증 안 됨 총괄표

1-1 ~ 1-4 문서의 "검증 안 됨" 항목과 1-5 신규 항목을 한 표로 모았다. "단위" 열은 항목이 처음 생긴 단위다. 검증됨으로 바뀐 항목(1-1의 `forced_quit` 행, 자동 실행 재조정 등)은 넣지 않았다.

| # | 단위 | 항목 | 이유 | 사람이 확인하는 한 줄 절차 |
|---|---|---|---|---|
| 1 | 1-1 | Windows 빌드·테스트(`cargo check/clippy/test`, `tauri build`) | [판정 2026-11-07: 검증됨(Windows) — 아래 결과 표로 해소; 이전 기록:] 로컬 Windows 빌드 불가(번들 SQLite C 헤더 없음), 장비 없음 | Windows PC에서 `cargo check/clippy/test`와 `pnpm tauri build`, 또는 원격 푸시 후 CI 확인 |
| 2 | 1-1 | Windows 절전·깨어남 감지 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] Windows 전원 이벤트 `Unsupported` | Windows PC에서 절전 후 깨워 `downtime_records`에 `sleep` 행이 생기는지 확인 |
| 3 | 1-1 | 데스크톱 GUI 실행, 트레이·메뉴바 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 에이전트에게 손쉬운 사용 권한 없음 | 앱을 실행해 트레이(메뉴바) 아이콘과 메뉴 항목을 눈으로 확인 |
| 4 | 1-1 | 실제 절전·깨어남 이벤트(`pmset sleepnow`) | [판정 2026-11-07: 맥에서 확인 필요] 실기기 절전 필요 | `pmset sleepnow` 후 깨워 `downtime_records`의 `sleep` 행과 `resumed` 표시 확인 |
| 5 | 1-1 | Cmd+Q 키 입력, 트레이 "종료" 클릭이 종료 게이트를 타는지 | [판정 2026-11-07: 맥에서 확인 필요] GUI 입력 권한 없음 | 공부 중 Cmd+Q와 트레이 종료를 눌러 거절·`appExit rejected_locked` 행 확인, 공부 밖에서는 종료되는지 확인 |
| 6 | 1-1 | 로그인 시점 실제 자동 실행, Windows Run 키 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 로그인·재부팅 필요 | 자동 실행을 켜고 재로그인해 앱이 떠 있는지 확인 |
| 7 | 1-1 | 단일 인스턴스 두 번째 실행 시 창 전면 표시 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] GUI 권한 | 앱 실행 중 `open -a`로 다시 열어 기존 창이 앞으로 오는지 확인 |
| 8 | 1-1 | 공부 중 설정 컨트롤 클릭의 UI 단위 동작 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 클릭 단위 UI 테스트 없음 | 공부 중 설정 화면에서 값을 바꿔 보고 거절 문구와 `setting_history` 행 확인 |
| 9 | 1-2 | 원격 CI 실행(`.github/workflows/ci.yml`) | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 원격 저장소 없음 | 원격에 푸시한 뒤 macOS·Windows 두 잡 통과 확인 |
| 10 | 1-2 | 알림 권한 팝업·실제 표시(macOS 첫 요청, Windows 앱 ID) | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] GUI 권한 | 앱 실행 후 휴식 종료 1분 전 알림이 뜨는지 확인 |
| 11 | 1-2 | 메뉴바 제목 가시성·폭, Windows 툴팁 갱신, 다크모드 가독성 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] GUI 확인 필요 | 앱을 실행하고 라이트·다크 모드에서 육안 확인 |
| 12 | 1-2 | 캘린더·히스토리 화면, 모드 변경 토스트와 되돌리기 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 클릭 단위 UI 테스트 없음 | 계획 생성 후 모드 변경하고 되돌리기까지 수동 수행 |
| 13 | 1-2 | 실제 절전·깨어남 후 계획 재확인 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실기기 절전 필요 | 계획 진행 중 절전 후 깨워 타이머 상태와 잠금이 맞는지 확인 |
| 14 | 1-2 | 로그인 시점 자동 시작 후 계획 자동 개시 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 로그인·재부팅 필요 | 재로그인 후 계획 시각에 잠금이 시작되는지 확인 |
| 15 | 1-2 | 비상 해제(abort_run) 화면 UX | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] GUI 확인 필요 | 잠금 중 해제 문장 입력을 끝까지 수행해 중단 처리와 기록 확인 |
| 16 | 1-3 | 실제 더미 앱의 정상 종료·강제 종료 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실제 종료는 실행하지 않음 | 시험 모드를 끄고 더미 앱을 차단 목록에 넣은 뒤 공부 구간에 실행해 종료되는지 확인 |
| 17 | 1-3 | Windows 프로세스 제어, 종료 우회, 재실행 | [판정 2026-11-07: 부분] Windows 빌드 불가, 장비 없음 | Windows PC에서 `cargo test`와 수동 시나리오 수행 |
| 18 | 1-3 | `class_addMethod`로 넣은 `applicationShouldTerminate:`가 라이브 앱에서 동작 | [판정 2026-11-07: 맥에서 확인 필요] GUI 앱 실행 필요 | 잠금 중 `osascript -e 'tell application "Study Manager" to quit'` 후 프로세스 생존과 `appExit rejected_locked` 행 확인 |
| 19 | 1-3 | 로그아웃·재시작·종료 이유 코드 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실기기 필요 | 잠금 중 로그아웃 후 재실행해 `forced_quit` 행 확인 |
| 20 | 1-3 | Cmd+Q(커스텀 메뉴)와 주입 게이트의 중복 호출 | [판정 2026-11-07: 맥에서 확인 필요] GUI 필요 | 잠금 해제 상태에서 Cmd+Q 한 번에 종료되는지 확인 |
| 21 | 1-3 | macOS LaunchAgent KeepAlive 재실행(`kill -9` 후) | [판정 2026-11-07: 맥에서 확인 필요] 로그인·launchd 실동작 필요 | 자동 실행을 켜고 재로그인 후 `kill -9`, 10초 안에 재실행과 `forced_quit` 기록 확인 |
| 22 | 1-3 | 해제 절차 UI(문장 입력, 붙여넣기 차단), 건너뛰기·긴급 패스 화면 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 클릭 단위 UI 테스트 없음 | 설정 화면에서 각 해제 경로를 직접 수행 |
| 23 | 1-3 | 차단 알림 실제 표시 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] GUI·권한 | 시험 모드에서 `would_terminate` 로그와 알림 확인 |
| 24 | 1-4 | 실제 Chrome에서 확장 로드 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] GUI·브라우저 조작 수단 없음 | `pnpm build:extension` 후 `chrome://extensions`에서 `extension/dist`를 압축해제 로드, 확장 ID가 `manoifclikefdhhomeklmdjbeknmloem`인지 확인 |
| 25 | 1-4 | 네이티브 메시징 매니페스트 등록과 연결 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 홈 디렉터리·브라우저에 설치하지 않음 | 설정의 "브라우저 연결 등록" 후 확장이 연결됨으로 바뀌고 `block_events`에 `extension_connected`가 생기는지 확인 |
| 26 | 1-4 | 차단 사이트 리다이렉트, 깜빡임, 서비스 워커 수명 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실제 브라우저 필요 | 실행 모드에서 공부 중 reddit.com 접속해 `blocked.html`로 바뀌는지, 30초 유휴 뒤에도 연결이 유지되는지 확인 |
| 27 | 1-4 | 확장 비활성화/삭제 시 끊김 기록과 경고 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실제 브라우저 필요 | 공부 중 확장을 끄고 설정 화면 경고와 `extension_disconnected` 행 확인 |
| 28 | 1-4 | 시크릿 모드 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 확장의 "시크릿 모드 허용"에 의존 | 허용 전후에 시크릿 창에서 차단 사이트 접속 |
| 29 | 1-4 | Edge·Brave 등 다른 Chromium 브라우저 | [판정 2026-11-07: 미구현] 등록 경로 미구현, 미설치 | 해당 브라우저의 NativeMessagingHosts 경로를 `platform/`에 추가한 뒤 연결 확인 |
| 30 | 1-4 | 프리셋 앱 번들 ID와 사이트 도메인 정확성 | [판정 2026-11-07: 맥에서 확인 필요] 이 PC에 해당 앱 없음, 일부 도메인은 작성자 추정 | 앱이 설치된 PC에서 `mdls -name kMDItemCFBundleIdentifier`로 확인하고 카탈로그 수정(`PRESET_VERSION` 올림) |
| 31 | 1-4 | 첫 실행 화면과 설정 화면 UX | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 클릭 단위 UI 테스트 없음 | 새 데이터 디렉터리로 실행해 카테고리 선택 화면과 쇼핑·웹툰이 꺼진 기본값 확인 |
| 32 | 1-4 | 공부 중 설정 잠금(UI 쪽: 프리셋·사이트·호스트 등록 해제) | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 클릭 단위 UI 테스트 없음 | 공부 중 설정에서 변경을 시도해 거절 메시지와 기록 확인 |
| 33 | 1-4 | Windows 전체(named pipe, 레지스트리 등록, 프로세스 경로 매칭) | [판정 2026-11-07: 부분] 빌드 불가, 장비 없음 | Windows PC에서 `cargo check/clippy/test`와 수동 시나리오 |
| 34 | 1-4 | 확장 ID가 실제 Chrome에서 같은 값으로 나오는지 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실제 Chrome 필요 | 항목 24에서 본 ID와 `ALLOWED_EXTENSION_IDS`가 같은지 비교 |
| 35 | 1-4→5 | 브라우저 차단의 일주일 실사용 오탐·미탐 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 시간 필요 | 일주일 사용 뒤 `block_events` 사이트 행을 검토 |
| 36 | 1-5 | 권한: 맨 앞 앱·입력 유휴 조회가 실제 앱에서 권한 팝업 없이 되는지, 창 제목 권한 안내 화면(`window_title_access`) 표시 | [판정 2026-11-07: 맥에서 확인 필요] 코드상 권한 불필요로 구현했으나 실제 팝업·시스템 설정 동작은 실행 못 함 | 처음 실행해 팝업이 뜨는지 보고, 안내 화면이 허용/거부 상태를 맞게 표시하는지 확인 |
| 37 | 1-5 | Windows 센서(`GetForegroundWindow`, `GetLastInputInfo`)와 Windows 컴파일 | [판정 2026-11-07: 부분] Windows 장비·빌드 불가 | Windows PC에서 공부 앱·딴짓 앱을 번갈아 켜고 `focus_events`에 이탈·복귀가 쌓이는지 확인 |
| 38 | 1-5 | 전역 단축키 충돌(`CommandOrControl+Alt+Shift+D`) | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 다른 앱과의 충돌은 환경 의존, GUI 필요 | 공부 중 단축키를 눌러 `focus_events`에 `self_report` 행이 1개 생기는지, 다른 앱(IDE 등)과 충돌하지 않는지 확인 |
| 39 | 1-5 | 단축키 등록 권한(macOS), 체크인 알림 표시 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] GUI·권한 | 공부 중 15~40분 사이에 체크인 알림이 뜨고 2분 안 응답이 기록되는지 확인 |
| 40 | 1-5 | 실사용 정확도: 이탈·복귀·멍때림(입력 10분 이상 없음) 판정 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실사용 필요 | 일주일 사용하며 대시보드의 인정 시간과 체감 집중 시간이 크게 어긋나는지 비교 |
| 41 | 1-5 | 전체 화면 비디오 시청 중 멍때림 오탐(입력 없음), 다중 모니터·전체 화면 앱, 화면 보호기·잠금 화면 중 값 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실기기 시나리오 필요 | 공부 앱 전체 화면으로 20분 두거나 모니터 둘로 써 보고 `focus_minutes`의 `idle_ms`·`away_ms`가 맞는지 확인 |
| 42 | 1-5 | 포인트 수치 밸런스(윈도우 +20P, 시작 보너스, 차감 30P, 레벨 곡선) | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 일주일 실사용이 있어야 판단 가능 | 일주일 뒤 레벨 추이가 너무 빠르거나 느리지 않은지 보고 `points/rewards.rs` 상수 조정 |
| 43 | 1-5 | 집중 측정 UI와 포인트·레벨 표시의 실제 화면 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 클릭 단위 UI 테스트 없음, GUI 권한 없음 | 공부 중 메인 화면의 집중 신호 카드와 세트 요약, 레벨 표시를 눈으로 확인 |
| 44 | 1-5 | 대시보드 레이아웃·가독성(오늘/이번 주/약점, 막대·히트맵, 다크모드) | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] GUI 확인 필요 | 대시보드 탭에서 세 보기를 열어 라이트·다크 모드로 글자 겹침과 색 대비 확인 |
| 45 | 1-5 | 대시보드 숫자가 실사용 기록과 맞는지(계획 대비 실제, 연속 달성일, 요일별) | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 손계산 골든 테스트만 통과, 실제 기록 비교 필요 | 하루 공부 뒤 `focus_minutes` 합계와 오늘 카드의 실제 시간을 비교 |
| 46 | 1-5 | 성장 그림·캐릭터 표시 | [판정 2026-11-07: 미구현] 그림 자산 없음(텍스트 단계 라벨만) | 4단계 상점 단위에서 결정 |
| 47 | 1-5 | **1단계 완료 기준**: 맥북에서 일주일 실사용 뒤 대시보드에 기록이 나온다 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 사람이 일주일 사용해야 함 | 아래 체크리스트 참고 |

### Windows PC 검증 결과 (2026-11-07)

환경: Windows 11 x64, rustc 1.99.1. 근거는 이 세션의 Windows 검증 기록뿐이다. 이 결과로 1단계가 완료되었다고 판단하지 않는다(완료 기준 체크리스트는 그대로 미체크).

| ID | 판정 | 명령 | 결과(종료코드·개수) | 비고 |
|---|---|---|---|---|
| 1 | 검증됨(Windows) | `cargo fmt --check`/`cargo clippy --all-targets -- -D warnings`/`cargo test` (src-tauri), `pnpm tauri build --bundles nsis` | 전부 종료 0. lib 1047 + llm_wire 4 + remote_wire 7 + doc-tests 3 통과, 실패 0. NSIS 설치 파일 5.03 MiB 생성(설치·실행 미수행) | Windows 11 네이티브 cargo fmt/clippy/test, `pnpm tauri build --bundles nsis`. 설치 파일은 실행하지 않음. 원격 CI는 #9 |
| 2 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | 실제 절전 필요 |
| 3 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | 트레이(Windows)·메뉴바(macOS) 육안 확인, GUI 미조작 |
| 4 | 맥에서 확인 필요 | 해당 명령 없음 (미수행) | 미수행 | pmset은 macOS 전용 |
| 5 | 맥에서 확인 필요 | 해당 명령 없음 (미수행) | 미수행 | Cmd+Q·macOS 트레이 종료 |
| 6 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | Windows 로그인/재부팅 필요 |
| 7 | 사람 확인(MANUAL-CHECKLIST) | 릴리스 exe 스모크 실행 | 판정 불가(inconclusive) | 릴리스 exe 스모크 실행은 기존 디버그 인스턴스가 단일 인스턴스 잠금을 잡아 판정 불가(inconclusive) |
| 8 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | GUI 클릭 필요 |
| 9 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | 원격 GitHub CI 필요 |
| 10 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | 알림 팝업 육안 확인 필요 |
| 11 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | 육안 확인 필요 |
| 12 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | GUI 클릭 필요 |
| 13 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | 실제 절전 필요 |
| 14 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | 로그인/재부팅 필요 |
| 15 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | GUI 확인 필요 |
| 16 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | 실제 종료 확인은 GUI·실행 필요 |
| 17 | 부분 | `cargo test` (src-tauri, Windows) | 종료 0. lib 1047 통과(cfg(unix) 테스트 제외) | Windows 네이티브 cargo test 통과, 단 cfg(unix) 테스트는 Windows에서 컴파일되지 않음. 실제 프로세스 종료·재실행 수동 시나리오는 미수행 |
| 18 | 맥에서 확인 필요 | 해당 명령 없음 (미수행) | 미수행 | osascript·macOS 전용 |
| 19 | 사람 확인(MANUAL-CHECKLIST) | 릴리스 exe 스모크 실행 | 판정 불가(inconclusive) | 로그아웃·재시작 필요. 릴리스 exe 스모크 실행은 inconclusive |
| 20 | 맥에서 확인 필요 | 해당 명령 없음 (미수행) | 미수행 | Cmd+Q·macOS |
| 21 | 맥에서 확인 필요 | 해당 명령 없음 (미수행) | 미수행 | LaunchAgent·launchd는 macOS 전용 |
| 22 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | GUI 클릭 필요 |
| 23 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | 알림 육안 확인 필요 |
| 24 | 사람 확인(MANUAL-CHECKLIST) | `pnpm build:extension` (로드는 미수행) | 종료 0 (빌드만) | Chrome+확장 필요. 이 PC에서는 `pnpm build:extension` 종료 0만 확인 |
| 25 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | Chrome 필요. Windows에서는 named pipe IPC 미구현(#33)이라 연결 불가 |
| 26 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | Chrome 필요 |
| 27 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | Chrome 필요 |
| 28 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | Chrome 필요 |
| 29 | 미구현 | 해당 명령 없음 (미수행) | 미수행 | Edge/Brave 매니페스트 경로 미구현 |
| 30 | 맥에서 확인 필요 | 해당 명령 없음 (미수행) | 미수행 | `mdls`는 macOS 전용 |
| 31 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | GUI 확인 필요 |
| 32 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | GUI 클릭 필요 |
| 33 | 부분 | `cargo clippy --all-targets -- -D warnings`/`cargo test` (src-tauri, Windows) | 종료 0. lib 1047 통과. named pipe IPC 미구현 | 네이티브 Windows cargo clippy/test 통과. named pipe 브라우저 IPC는 미구현(`bind_listener/connect`가 Unsupported 반환)이라 브라우저 연결은 동작 불가, 레지스트리 등록·프로세스 경로 매칭 수동 확인 미수행 |
| 34 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | Chrome 필요 |
| 35 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | 일주일 실사용 필요 |
| 36 | 맥에서 확인 필요 | 해당 명령 없음 (미수행) | 미수행 | macOS 권한 팝업·시스템 설정 동작 |
| 37 | 부분 | `cargo clippy --all-targets -- -D warnings`/`cargo test` (src-tauri, Windows) | 종료 0. lib 1047 통과 | Windows 네이티브 컴파일·clippy·cargo test 통과(`GetForegroundWindow`/`GetLastInputInfo` 컴파일 포함). 실제 센서 동작은 미확인 |
| 38 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | GUI·다른 앱과의 충돌 확인 필요 |
| 39 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | 알림·권한 육안 확인 필요(macOS 단축키 권한 포함) |
| 40 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | 일주일 실사용 필요 |
| 41 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | 실기기 시나리오 필요 |
| 42 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | 일주일 실사용 밸런스 판단 필요 |
| 43 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | 육안 확인 필요 |
| 44 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | 육안 확인 필요 |
| 45 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | 실제 기록 비교 필요 |
| 46 | 미구현 | 해당 명령 없음 (미수행) | 미수행 | 그림 자산 없음 |
| 47 | 사람 확인(MANUAL-CHECKLIST) | 해당 명령 없음 (미수행) | 미수행 | 일주일 실사용 필요 |

## 1단계 완료 기준 체크리스트 (사람이 확인)

- [ ] 맥북에서 일주일 동안 실제 계획으로 공부한다(시험 모드 또는 실행 모드).
- [ ] 대시보드 "오늘"에 다음 계획까지 남은 시간, 계획 대비 실제, 연속 달성일, 레벨이 나온다.
- [ ] 대시보드 "이번 주"에 요일별 계획 대비 실제, 달성률, 집중 점수, 과목별 시간이 나온다.
- [ ] 대시보드 "약점"에 딴짓 앱 순위, 첫 이탈까지 평균, 히트맵, 자주 밀리는 계획, 꺼진 시간이 나온다(해당 기록이 있을 때).
- [ ] 위 총괄표의 항목을 확인하고, 어긋난 것은 다음 단위로 넘긴다.

## 알려진 잔여 위험 (수용·추적)

- 앞선 단위의 위험은 각 단위 문서에 있다(Cmd+Q 경로, 확장 fail-open, DB 파일 삭제 등).
- 대시보드의 연속 달성일은 계획이 있었는데 앱이 꺼져 있어 실행 기록이 없던 날도 "계획했으나 못 한 날"로 센다(꺼진 시간은 약점 화면에 따로 보인다).
- 약점 화면의 딴짓 앱 순위는 차단 시도 기준이라 차단 목록에 없는 앱으로의 이탈은 히트맵과 "첫 이탈까지 평균"에만 반영된다. 차단 모드가 시험 모드일 때는 종료 요청 대신 시험 적중이 센다.
- 약점 기간(28일)과 "자주 밀리는 계획" 기준(2회)은 상수이며 실사용 뒤 조정한다.

## 다음 단계로 넘기는 항목

- 성장 탭과 주간 LLM 인사이트(3단계), 캐릭터·상점(4단계), 퀴즈 정답 보너스·세션 끝 위키 퀴즈(3단계).
- 창 제목 저장은 소비처가 생기는 3단계(위키 연결)까지 미룬다.
- 총괄표의 미확인 항목.
