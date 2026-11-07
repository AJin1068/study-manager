# 1단계-1 검증 기록 (Tauri 뼈대)

이 문서는 1단계-1의 검증 결과와 "검증 안 됨" 항목을 기록한다. 결과는 명령과 종료 코드로 적고, 실행하지 않은 것은 실행하지 않았다고 적는다.

## 환경

- macOS (darwin 23.4.0, arm64). Rust 1.99, Node LTS, pnpm 10.
- 저장소에는 커밋이 없어(모든 파일 untracked) 커밋 기준 CI 실행 이력이 없다.

## 실행한 검증 (macOS)

| 항목 | 명령 | 결과 |
|---|---|---|
| 타입 체크 | `pnpm tsc` | 종료 0 |
| 린트 | `pnpm lint` | 종료 0 |
| 프런트 테스트 | `pnpm test` | 25개 통과 (5 파일) |
| Rust 컴파일 | `cargo check --all-targets` | 종료 0 |
| Rust 린트 | `cargo clippy --all-targets -- -D warnings` | 종료 0 |
| Rust 테스트 | `cargo test` | 43개 통과 |
| macOS 번들 | `pnpm tauri build` | 종료 0, 번들 2개 생성 |
| 재시작 후 잠금 유지 | 빌드된 앱을 DB 기준으로 확인 | 검증함 (아래) |
| 잠금 행 삭제 후 재실행 | 행 삭제 뒤 재실행 | 시작 중단, 행을 되살리지 않음 (검증함) |

### 재시작·잠금 확인 (실제 DB 파일)

1. 새 DB에서 마이그레이션이 잠금 행을 만든다: `study_state = (1, 0, 0)`.
2. 행을 지우고 재실행하면 설정 단계에서 `study_state row is missing; lock state is unknown` 오류로 시작이 중단되고, 행은 다시 생기지 않는다.

## 검증 안 됨 (명시적 잔여 항목)

아래 항목은 이 환경에서 실행하지 못했거나 사용자 승인이 필요하다. 각 항목은 "검증 안 됨"이다.

| 항목 | 이유 | 관련 AC |
|---|---|---|
| Windows 빌드 (`cargo check`, `clippy`, `test`, `tauri build`) | [판정 2026-11-07: 검증됨(Windows) — 아래 「Windows PC 검증 결과」 표의 네이티브 빌드·시험 결과로 해소; 이전 기록:] 로컬 `cargo check --target x86_64-pc-windows-msvc`는 번들된 SQLite C 빌드에서 `stdlib.h`를 찾지 못해 실패한다(호스트에 Windows C 헤더 없음, 코드 결함이 아님). CI 매트릭스(`windows-latest`)에서만 검증 가능. | AC10 |
| Windows 절전·깨어남 감지 | [판정 2026-11-07: 미구현] Windows 전원 이벤트는 1단계-1에서 `Unsupported`. 실기기 검증은 S7a에서. | AC5 |
| 데스크톱 GUI 실행과 트레이·메뉴바 확인 | [판정 2026-11-07: 맥에서 확인 필요] 에이전트에게 macOS 손쉬운 사용(Accessibility) 권한이 없다 (`osascript` -1719). 사용자가 직접 확인해야 한다. | AC1 |
| 실제 절전·깨어남 이벤트 (`pmset sleepnow`) | [판정 2026-11-07: 맥에서 확인 필요] 실기기 절전이 필요. 단위 테스트는 가짜 이벤트만 다룬다. | AC5 |
| 강제 종료(kill) 후 `forced_quit` 행 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] **검증됨 (macOS)**: 실행 중 앱에 `kill -KILL`, 재실행 후 `downtime_records`에 `forced_quit` 1행, 해당 세션 `clean_exit=0`. 로그인 시점 실행은 별도 확인 대상이 아님. | AC6 |
| Cmd+Q·트레이 종료가 `ExitRequested`에 도달 | [판정 2026-11-07: 맥에서 확인 필요] **수정 후 결과 (macOS)**: 기본 Quit(`terminate:`)가 `ExitRequested`를 우회했다(tao 0.37.2 `app_delegate.rs:130`에 `applicationShouldTerminate` 없음). 이제 앱 메뉴의 커스텀 "종료"(CmdOrCtrl+Q)가 `app.exit(0)`으로 가서 같은 게이트를 탈다. 빌드·실행은 통과했지만 **Cmd+Q 키 입력 경로는 GUI 권한이 없어 검증 안 됨**. 트레이 "종료"도 동일 게이트이지만 GUI 클릭 필요로 검증 안 됨. **사용자 결정: (B) 수용.** `osascript ... to quit`(Apple Event 종료)는 공부 중에도 잠금을 우회한다(실측 확인). 이 경로는 1단계-1에서 수용하고 알려진 위험으로 남긴다. 다음 실행 때 세션은 `forced_quit`으로 기록된다. 해소는 1단계-3에서 하다(tao 델리게이트에 `applicationShouldTerminate` 런타임 주입 중 하나를 재검토). | AC9 |
| 자동 실행 등록 (LaunchAgent / Windows Run 키) | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] **macOS 재조정 검증됨**: DB에서 `autostart=false`로 바꾸고 재실행하면 `Study Manager.plist`가 제거되고 `setting_history`에 `reconcile`/`applied` 행이 생겼다(DB 우선). 로그인 시점의 실제 자동 실행과 Windows Run 키는 검증 안 됨(로그인·재부팅 필요). 테스트 중 생긴 plist는 확인 후 정리했다. | AC2 |
| 단일 인스턴스 두 번째 실행 (LaunchServices 경유) | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실행 확인함: 두 번째 `open`에도 프로세스는 하나였고 세션 수와 기록은 변하지 않았다. 창 전면 표시는 GUI 권한 제약으로 확인하지 못했다. | 안정성 |

### Windows PC 검증 결과 (2026-11-07)

환경: Windows 11 x64, rustc 1.99.1 (x86_64-pc-windows-msvc), node 26.13, pnpm 10.34.9.

| ID | 판정 | 명령 | 결과(종료코드·개수) | 비고 |
|---|---|---|---|---|
| 1-1-V1 Windows 빌드 | 검증됨(Windows) | `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (src-tauri); `pnpm tauri build --bundles nsis` | 모두 종료 0. `cargo test`: lib 1047 + llm_wire 4 + remote_wire 7 + doc 3 통과, 실패 0. NSIS 설치 파일 5.03 MiB 생성 | 이전 결과(SQLite 헤더 부재로 `cargo check` 실패)는 네이티브 Windows로 대체됨. 설치 파일은 실행하지 않았고 GUI는 보지 않았다. `cfg(unix)` 테스트는 Windows에서 컴파일되지 않는다. 수정 커밋 a22bb03, cd88e14 |
| 1-1-V2 Windows 절전·깨어남 | 미구현 | 해당 없음 | 해당 없음 | 코드가 `Unsupported`; 실기기 검증은 S7a |
| 1-1-V3 GUI·트레이·메뉴바 | 맥에서 확인 필요 | 해당 없음 | 해당 없음 | 메뉴바는 macOS 전용. Windows 트레이 GUI는 이 PC에서 보지 않았다 |
| 1-1-V4 `pmset sleepnow` | 맥에서 확인 필요 | 해당 없음 | 해당 없음 | macOS 전용 |
| 1-1-V5 강제 종료 `forced_quit` | 사람 확인(MANUAL-CHECKLIST) | 릴리스 exe 스모크 실행 | 불확정 | 기존 디버그 인스턴스가 단일 인스턴스 잠금을 쥐고 있어 첫 실행·`forced_quit` 순서를 관찰하지 못했다. DB 파일 `%APPDATA%\com.studymanager.app\study-manager.sqlite`는 존재 |
| 1-1-V6 Cmd+Q·트레이 종료 | 맥에서 확인 필요 | 해당 없음 | 해당 없음 | macOS 전용 키 입력 경로 |
| 1-1-V7 자동 실행 등록 | 사람 확인(MANUAL-CHECKLIST) | 해당 없음 | 해당 없음 | Windows Run 키는 로그인·재부팅 필요. LaunchAgent 로그인 시점 실행은 macOS 확인 |
| 1-1-V8 단일 인스턴스 창 전면 | 사람 확인(MANUAL-CHECKLIST) | 해당 없음 | 해당 없음 | GUI 육안 확인 필요 |
| 1-1-V9 설정 잠금 UI 클릭(본문) | 사람 확인(MANUAL-CHECKLIST) | 해당 없음 | 해당 없음 | 백엔드 거절은 `cargo test`로 이미 검증됨. 클릭 단위는 GUI |

## 알려진 잔여 위험 (수용·추적)

- **D7 테스트 토글**: `set_study_state_test`가 릴리스 빌드에 노출된다. 승인된 결정이며, 1단계-3 전에 제거 여부를 정한다. 이 토글로 공부 잠금을 해제할 수 있다.
- **DB 파일 삭제**: 공부 중 DB 파일을 통째로 지우면 새 잠금 행(`studying=0`)이 생긴다. 행 삭제는 막지만 파일 삭제는 막을 수 없다. 수용된 잔여 위험.
- **잃은 깨어남 신호**: 깨우기 신호가 빠지면 잠들기 신호 시각부터 첫 점검 시각까지를 추정(`estimated=1`)으로 기록하고 잠들기 상태를 닫는다. 추정 뒤 깨어 있는 점검은 행을 만들지 않는다. 실제 수면이 90초 이상이면 오차는 하트비트 간격(30초) 이내이고, 더 짧은 수면은 최대 약 120초 과대 추정될 수 있다. 추정 뒤 도착한 깨우기 신호는 기록하지 않고 로그(`did_wake_after_estimate`)로만 남긴다.
- **"강제 종료 허용"**: 기록만 한다. 실제 집행은 1단계-3.
- **계획 재확인 (AC5)**: 이번 단계는 `resumed` 이벤트와 화면 표시까지만 구현했다. 계획 조회는 1단계-2 범위이므로 "계획 재확인"은 완료가 아니라 1단계-2로 넘어간다.
- **공부 중 설정 잠금 UI (AC3)**: 설정 컨트롤의 `disabled`를 제거해 잠금 중 변경 시도가 백엔드까지 가고, 백엔드가 `rejected_locked` 행을 남기도록 고쳤다(PLAN.md 184행 준수). 백엔드 거절 기록은 Rust 테스트로 검증했다. 클릭 단위 UI 테스트는 테스트 라이브러리가 없어 추가하지 않았으며 "검증 안 됨"이다. [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 잠금 중 UI 안내 문구는 유지한다.
