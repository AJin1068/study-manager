# 1단계-3 검증 기록 (앱 차단 코어)

결과는 명령과 종료 코드로 적고, 실행하지 않은 것은 실행하지 않았다고 적는다. 검증 안 됨 항목은 다음 단위를 막지 않는다.

## 이전 단위 이월 확인

- 1-2의 검증 안 됨 표는 그대로 유효(GUI·Windows·실기기 항목). 수동 토글(D7)은 2-S4에서 제거됨. 비상 해제(abort_run)는 3-S4 해제 절차로 일반화됨.
- Cmd+Q/Apple Event 우회(1-1 수용)는 3-S5에서 `applicationShouldTerminate` 주입으로 대응(실제 동작은 아래 검증 안 됨).

## 실행한 검증 (macOS, 3-S6 완료 시점의 전체 트리)

| 항목 | 명령 | 결과 |
|---|---|---|
| 타입 체크 | `tsc --noEmit` | 종료 0 |
| 린트 | `eslint .` | 종료 0 |
| 프런트 테스트 | `vitest run` | 98개 통과 |
| Rust 린트 | `cargo clippy --all-targets -- -D warnings` | 종료 0 |
| Rust 테스트 | `cargo test` | 208개 통과 (가짜 `ProcessControl`로 정상 종료→강제 종료 순서, 시험 모드 무종료, 보호 목록, 휴식 중 해제, 개인정보 스키마 가드 포함) |

시험 모드에서 실제 프로세스가 종료되지 않음은 가짜 `ProcessControl` 호출 기록으로 검증했다. 실제 프로세스에 대한 종료는 실행하지 않았다.

## 검증 안 됨 (명시적 잔여 항목)

| 항목 | 이유 | 사람이 확인하는 방법 |
|---|---|---|
| 실제 더미 앱 정상 종료·강제 종료 (macOS/Windows) | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실제 종료는 실행하지 않음 | 시험 모드를 끄고 더미 앱을 차단 목록에 넣어 공부 구간에 실행 |
| Windows 전체(프로세스 제어, 종료 우회, 재실행) | [판정 2026-11-07: 부분] 로컬 Windows 빌드 불가(SQLite C 헤더), 장비 없음 | Windows PC에서 `cargo check/clippy/test`, 수동 시나리오 |
| `class_addMethod`로 `applicationShouldTerminate:` 주입이 라이브 tao 델리게이트에서 동작 | [판정 2026-11-07: 맥에서 확인 필요] GUI 앱 실행 필요 | 잠금 중 `osascript -e 'tell application "Study Manager" to quit'`로 프로세스가 살아 있고 `appExit rejected_locked` 행이 남는지 확인 |
| 로그아웃/재시작/종료 Apple Event 이유 코드 | [판정 2026-11-07: 맥에서 확인 필요] 실기기 필요 | 잠금 중 로그아웃 후 재실행 시 `forced_quit` 행 확인 |
| Cmd+Q(커스텀 메뉴 `app.exit(0)`)와 주입 게이트의 중복 호출 | [판정 2026-11-07: 맥에서 확인 필요] GUI 필요 | 잠금 해제 상태에서 Cmd+Q가 한 번에 종료되는지 확인 |
| macOS LaunchAgent KeepAlive 재실행 (`kill -9` 후) | [판정 2026-11-07: 맥에서 확인 필요] 로그인·launchd 실동작 필요 | 자동 실행 켜고 재로그인 후 `kill -9`, 10초 내 재실행·`forced_quit` 기록 확인 |
| 해제 절차 UI(문장 입력, 붙여넣기 차단), 건너뛰기·긴급 패스 화면 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 클릭 단위 UI 테스트 없음, GUI 권한 없음 | 설정 화면에서 각 해제 경로 수행 |
| 차단 알림 실제 표시 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] GUI/권한 | 시험 모드 `would_terminate` 로그와 알림 확인 |

### Windows PC 검증 결과 (2026-11-07)

환경: Windows 11 x64, rustc 1.99.1 (x86_64-pc-windows-msvc), node 26.13, pnpm 10.34.9.

| ID | 판정 | 명령 | 결과(종료코드·개수) | 비고 |
|---|---|---|---|---|
| 1-3-V1 실제 더미 앱 정상·강제 종료 | 사람 확인(MANUAL-CHECKLIST) | 해당 없음 | 해당 없음 | 가짜 `ProcessControl` 테스트만 `cargo test`에서 통과. 실제 프로세스 종료는 실행하지 않았다 |
| 1-3-V2 Windows 전체(프로세스 제어·종료 우회·재실행) | 부분 | `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (src-tauri) | 모두 종료 0. `cargo test`: lib 1047 + llm_wire 4 + remote_wire 7 + doc 3 통과, 실패 0 | 컴파일·단위 테스트만 확인(이전 "Windows 빌드 불가"는 대체됨). Windows 재실행 감시는 `Unsupported`로 미구현. 실제 프로세스 제어·종료 우회 수동 시나리오는 미실행 |
| 1-3-V3 `applicationShouldTerminate:` 주입 | 맥에서 확인 필요 | 해당 없음 | 해당 없음 | macOS 전용 |
| 1-3-V4 로그아웃/재시작/종료 Apple Event | 맥에서 확인 필요 | 해당 없음 | 해당 없음 | macOS 실기기 |
| 1-3-V5 Cmd+Q 중복 호출 | 맥에서 확인 필요 | 해당 없음 | 해당 없음 | macOS 전용 |
| 1-3-V6 LaunchAgent KeepAlive 재실행 | 맥에서 확인 필요 | 해당 없음 | 해당 없음 | macOS 전용 |
| 1-3-V7 해제 절차 UI·건너뛰기·긴급 패스 화면 | 사람 확인(MANUAL-CHECKLIST) | 해당 없음 | 해당 없음 | GUI 클릭 필요 |
| 1-3-V8 차단 알림 실제 표시 | 사람 확인(MANUAL-CHECKLIST) | 해당 없음 | 해당 없음 | GUI·권한 확인 필요 |

## 알려진 잔여 위험 (수용·추적)

- 주입 실패 시 Apple Event 종료는 잠금을 우회하지만 `clean_exit`가 설정되지 않아 다음 실행에서 `forced_quit`로 기록된다.
- Windows 로그오프 거부는 사용자를 가둘 수 있어 구현하지 않았다. Windows 재실행 감시는 `Unsupported`.
- macOS 자동 실행은 플러그인 대신 자체 LaunchAgent 작성기로 바꿨다. 자동 실행이 꺼져 있으면 재실행 감시도 없다.
- 시험 모드가 기본값이며 실행 모드 전환은 설정에서 명시적으로만 한다.

## 다음 단위(1단계-4)로 넘기는 항목

- `block_events`(앱·사이트·확장 상태 공용 형식), `study_passes`, `enforcement_mode` 계약.
