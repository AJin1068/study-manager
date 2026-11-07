# 1단계-4 검증 기록 (브라우저 확장, 앱 연결, 차단 프리셋)

결과는 명령과 종료 코드로 적는다. 검증 안 됨 항목은 다음 단위를 막지 않는다.

## 통신 방식 결정 요약 (4-S0)

**native messaging + 로컬 IPC.** Chrome이 호스트 프로세스(앱 바이너리, `chrome-extension://<id>/` 인자로 구분)를 띄우고, 호스트가 Chrome stdio와 앱 데이터 디렉터리의 Unix 소켓(`0600`) 사이에서 프레임을 중계한다. 호스트가 설치별 토큰과 호출 확장 ID를 첫 프레임으로 붙이므로 확장은 비밀을 다루지 않는다. localhost 서버는 아무 로컬 프로세스나 웹 페이지가 접근할 수 있고 토큰 전달 경로가 마땅치 않아 기각했다. 근거와 반증 시도(왕복 지연 중앙값 약 14 µs, 앱 미실행 시 오류 응답, 잘린/과대 프레임)는 `docs/stage-1-4-transport-decision.md`.

## 이번 단위에서 만든 것

| 스토리 | 내용 |
|---|---|
| 4-S4 | `presets` 모듈: 버전 있는 번들 카탈로그(PLAN 표 그대로, 쇼핑·웹툰만 꺼짐, 음악은 항상 허용), `block_entries(source='preset', category)`로 구체화, 카테고리 토글(잠금 중 거절·`setting_history`), 첫 실행 화면, 카탈로그 갱신 시 사용자 항목·기존 on/off 보존, 프리셋 항목 삭제 금지(카테고리로만 끄기). 사이트 차단 항목 추가, 엄격 모드·허용 목록·유튜브 공부 재생목록/채널 편집(`site_rules::edit`, 잠금 중 거절). |
| 4-S5 | 확장 ID를 `ALLOWED_EXTENSION_IDS`에 고정(이전에는 빈 목록이라 모든 연결이 거절됐다), 호스트 매니페스트 등록/해제 명령(해제는 공부 중 거절), 앱 시작 시 닫히지 않은 연결 정리, 확장 끊김 구간을 읽을 때 파생하는 "브라우저 감시 불가 구간"(`browser_link::coverage`), 설정 화면의 연결 상태·끊김 경고, Rust 종단 테스트(가짜 브라우저 링크). |

## 에이전트가 실행한 범위 테스트 (macOS)

| 항목 | 명령 | 결과 |
|---|---|---|
| 프리셋 | `cargo test --lib presets` | 12개 통과 |
| 사이트 규칙·편집 | `cargo test --lib site_rules` | 19개 통과 |
| 브라우저 링크(종단 3개, 와이어 형식 포함) | `cargo test --lib browser_link` | 44개 통과 |
| 감시 불가 구간·호스트 등록 | `cargo test --lib browser_link::coverage` / `browser_link::register` | 5개 / 1개 통과 |
| 앱 차단(프리셋 삭제 금지 반영) | `cargo test --lib enforce` | 31개 통과 |
| 개인정보 스키마 가드 | `cargo test --lib privacy_schema` | 1개 통과 |
| 공유 픽스처(Rust 쪽) | `cargo test --lib browser_wire` | 1개 통과 |
| 프런트 신규 테스트 | `vitest run src/api/browser.test.ts src/pages/browserView.test.ts` | 9개 통과 |
| 타입 체크, 린트(`src`) | `tsc --noEmit`, `eslint src` | 종료 0, 종료 0 |

## 전체 검증 (리더가 실행)

| 항목 | 명령 | 결과 |
|---|---|---|
| 타입 체크 | `pnpm tsc` | 종료 0 (`tsc --noEmit`) |
| 린트 | `pnpm lint` | 종료 0 (`eslint .`) |
| 프런트 테스트 | `pnpm test` | 132개 통과 |
| 확장 빌드 | `pnpm build:extension` | 종료 0 (`extension/dist` 생성) |
| Rust 컴파일 | `cargo check --all-targets` (src-tauri) | 종료 0 |
| Rust 린트 | `cargo clippy --all-targets -- -D warnings` | 종료 0 |
| Rust 테스트 | `cargo test` | 289개 통과 |
| 번들 | `pnpm tauri build` | 종료 0, .app/.dmg 2개 생성 |

## 종단 시나리오 (Rust 테스트, 가짜 브라우저 링크)

`browser_link::e2e_tests`는 실제 타이머(20:00 계획, 잠금 보유), 실제 프리셋 선택, 실제 사이트 규칙, 실제 `block_events`를 쓰고 브라우저 쪽만 스크립트 프레임으로 대신한다.

1. 탭 전환 → 판단 → `block_events` → 응답: 차단 사이트(reddit.com, 등록하지 않은 유튜브 영상)는 `block`, 쇼핑(꺼진 카테고리)·일반 문서는 `allow`, 등록한 재생목록은 `allowed_study_video`, `chrome://`은 `not_web`. 행은 도메인과 고정 코드만 갖고 `run_id`/`segment_id`가 붙으며, `count_attempts`(요약의 `attemptCount`)에 합산된다. URL 경로·쿼리·조각·영상 ID·재생목록 ID는 행과 로그에 없다.
2. 잠금 중 규칙 편집(허용 목록 변경, 사이트 추가, 카테고리 끄기, 호스트 매니페스트 해제)은 모두 `rejected_locked`로 기록되고 이후 판단이 그대로다.
3. 연결이 끊기면(확장 비활성화·삭제와 같은 EOF) `extension_disconnected(closed)`가 남고 상태가 끊김으로 바뀌며, 그 공부 구간이 감시 불가 시간으로 계산된다.

## 우회 경로 표 (4-S5 갱신)

| 경로 | 처리 | 잔여 |
|---|---|---|
| 확장 비활성화/삭제 | 호스트가 종료되어 앱이 EOF를 보고 `extension_disconnected` 기록, 감시 불가 구간 통계, 설정 화면 경고 | fail-open(D-22). 비활성화와 삭제는 앱이 구분하지 못한다. 공부 중 알림 팝업은 만들지 않았다 |
| 앱이 죽은 채 연결 기록이 열려 있음 | 다음 시작 때 열린 연결마다 `extension_disconnected(app_restart)`를 쓴다 | 죽어 있던 구간은 연결 종료 시각을 알 수 없어 다음 시작 시각까지 감시됨으로 계산된다 |
| 호스트 매니페스트 삭제 | 앱의 해제 명령은 공부 중 거절. 파일을 직접 지우는 것은 막지 못함 | 수동 삭제, 수용 |
| 규칙·프리셋 편집으로 차단 해제 | 모두 잠금 중 거절 + 기록 | 공부 밖 시간에는 편집 가능 |
| 시크릿 모드·다른 브라우저 | 확장이 허용된 창에서만 동작 | 검증 안 됨 (아래) |
| 확장이 보내지 않는 URL 변경 | 이벤트당 1회 전송 규칙에 의존 | 실사용 오탐/미탐 검증 안 됨 |
| 유튜브 일반 `/watch?v=` 영상의 채널 허용 | URL에 채널이 없어 채널 단위 허용은 채널 페이지와 재생목록 링크만 해당 | 제목 기반 판단은 3단계 |

## 브라우저별 등록 확인 결과

| 브라우저/OS | 등록 위치 | 상태 |
|---|---|---|
| Chrome / macOS | `~/Library/Application Support/Google/Chrome/NativeMessagingHosts/com.studymanager.app.browser.json` | [판정 2026-11-07: 맥에서 확인 필요] 매니페스트 내용과 쓰기/삭제는 임시 디렉터리 테스트로 확인. 실제 Chrome이 읽는지는 **검증 안 됨** (홈 디렉터리에 설치하지 않음) |
| Edge, Brave 등 다른 Chromium / macOS | 코드에 경로 없음 | [판정 2026-11-07: 미구현] **검증 안 됨** (미구현) |
| Chrome / Windows | `HKCU\Software\Google\Chrome\NativeMessagingHosts\…` | [판정 2026-11-07: 미구현] **검증 안 됨** (미구현, `Unsupported`) |
| Firefox, Safari | 해당 없음 | 후속 단계 |

## 프리셋 식별자 (모두 이 PC에서 확인하지 못함)

`/Applications`에 아래 앱이 하나도 설치되어 있지 않아 번들 ID를 실제 설치본(`mdls -name kMDItemCFBundleIdentifier`)으로 확인할 수 없었다. 값은 작성자의 기억에 의존하며 **검증 안 됨**이다. 틀리면 해당 앱이 차단되지 않을 뿐 다른 앱을 끝내지는 않는다(정확히 일치해야 매칭). 사용자는 설정의 "실행 중인 앱에서 고르기"로 실제 ID를 직접 추가해 덮어쓸 수 있다.

| 카테고리 | 앱 | 번들 ID (미검증) |
|---|---|---|
| 메신저 | 카카오톡 | `com.kakao.KakaoTalkMac` |
| 메신저 | 디스코드 | `com.hnc.Discord` |
| 메신저 | 텔레그램 | `ru.keepcoder.Telegram` (Telegram Desktop은 `org.telegram.desktop`일 수 있음) |
| 영상 | 넷플릭스 | `com.netflix.Netflix` (macOS 네이티브 앱 존재 여부부터 불확실) |
| 영상 | 쿠팡플레이, 티빙 | **카탈로그에 없음**: 식별자를 알 수 없고 macOS 앱 존재도 확인 못 함. 사용자가 직접 추가 |
| 게임 | 스팀 | `com.valvesoftware.steam` |
| 게임 | 라이엇 클라이언트 | `com.riotgames.RiotGames.RiotClient` |
| 게임 | 배틀넷 | `net.battle.app` |
| 게임 | 에픽게임즈 | `com.epicgames.EpicGamesLauncher` |

사이트 도메인: PLAN 표의 도메인은 그대로 썼다. 표에 도메인이 없는 항목은 작성자가 정했고 **검증 안 됨**이다: 웹 카톡 `web.kakao.com`, 쿠팡 `coupang.com`, 29CM `29cm.co.kr`, 네이버 웹툰 `comic.naver.com`. Windows는 앱을 실행 파일 전체 경로로 매칭하므로 프리셋 앱 항목이 동작하지 않는다(Windows 경로 목록은 2단계).

## 검증 안 됨 (명시적 잔여 항목, 사람이 확인)

| 항목 | 이유 | 사람이 확인하는 방법 |
|---|---|---|
| 실제 Chrome에서 확장 로드(압축해제 로드) | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 에이전트에 GUI·브라우저 조작 수단 없음, 사용자 브라우저에 설치하지 않음 | `pnpm build:extension` 후 `chrome://extensions` → 개발자 모드 → 압축해제된 확장 로드 → `extension/dist`. 확장 ID가 `manoifclikefdhhomeklmdjbeknmloem`인지 확인 |
| 네이티브 메시징 매니페스트 등록과 연결 | [판정 2026-11-07: 맥에서 확인 필요] 홈 디렉터리/브라우저 설치 금지 | 앱 실행 → 설정의 "브라우저 연결 등록" → Chrome에서 확장 아이콘이 연결됨으로 바뀌고 `block_events`에 `extension_connected`가 생기는지 확인 |
| 차단 사이트 리다이렉트, 깜빡임, 서비스 워커 수명 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실제 브라우저 필요 | 실행 모드에서 공부 중 reddit.com 접속 → `blocked.html`로 바뀌는지, 30초 유휴 후에도 연결이 유지되는지 확인 |
| 확장 비활성화/삭제 시 끊김 기록과 경고 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실제 브라우저 필요 (앱 쪽 기록은 가짜 링크로 검증함) | 공부 중 확장을 끄고 설정 화면 경고, `extension_disconnected` 행, 감시 불가 구간 값을 확인 |
| 시크릿 모드 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 확장이 "시크릿 모드에서 허용"일 때만 동작 | 허용 전/후에 시크릿 창에서 차단 사이트 접속 |
| 다른 Chromium 브라우저(Edge, Brave 등) | [판정 2026-11-07: 미구현] 등록 경로 미구현, 미설치 | 경로 확인 후 `platform/`에 추가 |
| 프리셋 앱 식별자 정확성 | [판정 2026-11-07: 맥에서 확인 필요] 이 PC에 해당 앱 없음 | 위 표의 앱을 설치한 PC에서 번들 ID를 `mdls`로 확인하고 카탈로그 수정(`PRESET_VERSION` 올림) |
| 첫 실행 화면과 설정 화면 UX | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 클릭 단위 UI 테스트 없음, GUI 권한 없음 | 새 데이터 디렉터리로 앱 실행 → 첫 실행 카테고리 선택 화면, 쇼핑·웹툰이 꺼진 기본값, 설정에서 토글 |
| 공부 중 설정 잠금(UI 쪽) | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 클릭 단위 UI 테스트 없음 | 공부 중 설정에서 프리셋/사이트/호스트 등록 해제 시도 → 거절 메시지와 기록 확인 (Rust 쪽 거절은 테스트함) |
| Windows 전체 (named pipe, 레지스트리 등록, 프로세스 경로 매칭, 빌드) | [판정 2026-11-07: 부분] 로컬 Windows 빌드 불가(번들 SQLite C 헤더), 장비 없음 | Windows PC에서 `cargo check/clippy/test`, 수동 시나리오 |
| 일주일 실사용 오탐/미탐 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 시간 필요 | 시험 모드로 일주일 사용 후 `block_events` 검토 |

### Windows PC 검증 결과 (2026-11-07)

환경: Windows 11 x64, rustc 1.99.1 (msvc). 위 맥 결과 표와 별개로 이 PC에서 실행한 범위만 적는다.

| ID | 판정 | 명령 | 결과(종료코드·개수) | 비고 |
|---|---|---|---|---|
| V1 Chrome 압축해제 확장 로드 | 사람 확인(MANUAL-CHECKLIST) | `pnpm build:extension` | 종료 0 (빌드만) | 브라우저에 로드하지 않음. 확장 ID 확인은 사람 |
| V2 네이티브 메시징 등록·연결 | 맥에서 확인 필요 | 해당 없음 | 해당 없음 | macOS 경로 기준. Windows는 named pipe·레지스트리 등록 미구현(`Unsupported`)이라 V10 참조 |
| V3 차단 리다이렉트·서비스 워커 수명 | 사람 확인(MANUAL-CHECKLIST) | 해당 없음 | 해당 없음 | 실제 브라우저 필요 |
| V4 확장 비활성화/삭제 시 끊김 | 사람 확인(MANUAL-CHECKLIST) | 해당 없음 | 해당 없음 | 앱 쪽 기록은 `cargo test`(Rust 가짜 링크)에 포함 |
| V5 시크릿 모드 | 사람 확인(MANUAL-CHECKLIST) | 해당 없음 | 해당 없음 | |
| V6 Edge/Brave | 미구현 | 해당 없음 | 해당 없음 | 등록 경로 코드 없음 |
| V7 프리셋 앱 번들 ID | 맥에서 확인 필요 | 해당 없음 | 해당 없음 | `mdls`는 macOS 전용. Windows는 실행 파일 전체 경로 매칭이라 프리셋 앱 미동작 |
| V8 첫 실행·설정 UX | 사람 확인(MANUAL-CHECKLIST) | 해당 없음 | 해당 없음 | GUI 미조작. 릴리스 exe 스모크 실행은 기존 디버그 인스턴스의 단일 인스턴스 잠금 때문에 판단 불가 |
| V9 공부 중 설정 잠금(UI) | 사람 확인(MANUAL-CHECKLIST) | 해당 없음 | 해당 없음 | Rust 쪽 거절은 `cargo test`에 포함 |
| V10 Windows 전체 | 부분 | `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` (src-tauri, 네이티브 x86_64-pc-windows-msvc) / `pnpm tauri build --bundles nsis` | 종료 0 / 종료 0 / lib 1047 + llm_wire 4 + remote_wire 7 + doc-tests 3 통과, 실패 0 / 종료 0 (설치 파일 5.03 MiB, 실행 안 함) | 이전 "로컬 Windows 빌드 불가" 결과는 대체됨. `cfg(unix)` 테스트(소켓·심볼릭 링크)는 Windows에서 컴파일되지 않음. named pipe·레지스트리 등록·프로세스 경로 프리셋은 미구현이라 연결 자체는 검증 불가(a22bb03은 clippy dead_code만 수정) |
| V11 일주일 실사용 | 사람 확인(MANUAL-CHECKLIST) | 해당 없음 | 해당 없음 | |
| R1 Chrome/macOS 등록 | 맥에서 확인 필요 | 해당 없음 | 해당 없음 | |
| R2 Edge, Brave/macOS | 미구현 | 해당 없음 | 해당 없음 | |
| R3 Chrome/Windows 레지스트리 | 미구현 | 해당 없음 | 해당 없음 | `Unsupported`. 구현 후 사람 확인 필요 |

## 알려진 잔여 위험 (수용·추적)

- 확장이 없거나 끊기면 사이트는 열린다(D-22). 앱은 기록과 경고로만 대응한다.
- `ALLOWED_EXTENSION_IDS`는 매니페스트 `key`에서 SHA-256으로 계산한 값이며(테스트 스위트 밖에서 확인), 실제 Chrome이 같은 ID를 주는지는 검증 안 됨. 어긋나면 모든 연결이 `unknown_extension`으로 거절된다.
- 프리셋 항목은 삭제할 수 없고 카테고리로만 끈다. 카탈로그 갱신은 새 항목만 추가하며, 사용자가 이미 같은 패턴을 직접 넣었다면 그 행이 사용자 행으로 남는다.
- 엄격 모드 기본 허용 목록은 최소 집합(D-23, 사용자 확인 대기).
- (W3) 이벤트 속도 제한(10초당 60건)은 열린 채로 실패한다(fail-open). 한도를 넘으면 차단 사이트도 `allow`(`rate_limited`)가 되며, 페이지가 `history.pushState`를 반복 호출하면 한도를 소진시킬 수 있다. D-22(실패 시 허용)에 따라 수용하고 추적한다. 수정하지 않았다.
- (W5) 확장의 `hello` 버전은 형식만 검사한다(`is_valid_version`). 프로토콜 버전 게이트가 없어 `0.0.1` 같은 다른 버전도 받아들인다. 수정하지 않았다.
- (W6) 해제 문장 입력의 붙여넣기 차단은 UI에만 있다. 코어는 문장이 정확히 같은지만 확인하고 입력 시간(`typing_ms`는 기록만)은 검증하지 않으므로, Tauri 명령을 직접 호출하면 제출할 수 있다.

## 다음 단위(1단계-5)로 넘기는 항목

- `browser_link::coverage`(감시 불가 구간)와 `block_events`의 사이트 행을 집중 측정의 브라우저 판단 입력으로 연결.
- 브라우저 판단이 있으면 그것을 우선한다는 규칙(5-S1)은 `TabJudge` 결과와 확장 연결 상태를 입력으로 쓴다.
