# 1단계-2 검증 기록 (캘린더 계획·타이머)

이 문서는 1단계-2의 검증 결과와 "검증 안 됨" 항목을 기록한다. 결과는 명령과 종료 코드로 적고, 실행하지 않은 것은 실행하지 않았다고 적는다. 검증 안 됨 항목은 다음 단위를 막지 않는다.

## 환경

- macOS (darwin 23.4.0, arm64). Node는 `$HOME/.local/node/bin`(pnpm 포함)을 PATH에 추가해 실행.
- 저장소 커밋 이력은 있으나 1단계-2 변경은 모두 미커밋(작업 트리).

## 실행한 검증 (macOS, 2-S6 완료 시점의 전체 트리)

| 항목 | 명령 | 결과 |
|---|---|---|
| 타입 체크 | `tsc --noEmit` | 종료 0 |
| 린트 | `eslint .` | 종료 0 |
| 프런트 테스트 | `vitest run` | 86개 통과 (11 파일) |
| Rust 린트 | `cargo clippy --all-targets -- -D warnings` | 종료 0 |
| Rust 테스트 (`privacy_schema_guard` 포함) | `cargo test` | 141개 통과 |
| 수동 토글 제거 | `search set_study_state_test` | 소스 잔존 0건 (M1 SQL CHECK의 `test_toggle`만 남음, 마이그레이션 불변 규칙) |

스토리별 중간 점검: 2-S0 후 Rust 47 / TS 25, 2-S1·S2 후 Rust 77 / TS 42, 2-S3·S4 후 Rust 117 / TS 73, 2-S5·S6 후 Rust 141 / TS 86 (모두 통과).

## CI

- `.github/workflows/ci.yml`(macos-latest, windows-latest 매트릭스)는 정의만 있고 원격 실행은 **검증 안 됨**. [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 사람: 원격에 푸시한 뒤 두 잡 통과를 확인.

## 검증 안 됨 (명시적 잔여 항목)

| 항목 | 이유 | 사람이 확인하는 방법 |
|---|---|---|
| Windows 빌드·테스트 | [판정 2026-11-07: 검증됨(Windows) — 아래 「Windows PC 검증 결과」 표의 네이티브 빌드·시험 결과로 해소; 이전 기록:] `cargo check --target x86_64-pc-windows-msvc`가 번들 SQLite C 빌드에서 `stdlib.h`를 못 찾아 실패(호스트 헤더 부재, 코드 결함 아님) | Windows PC에서 `cargo check/clippy/test`, 또는 원격 푸시 후 CI |
| 원격 CI 실행 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 원격 저장소 없음 | 푸시 후 두 잡 확인 |
| 알림 권한 팝업·실제 표시 (macOS 첫 요청, Windows 앱 ID) | [판정 2026-11-07: 맥에서 확인 필요] GUI 권한 없음 | 앱 실행 후 휴식 종료 1분 전 알림 확인 |
| 메뉴바 제목 가시성·폭, Windows 툴팁 갱신, 다크모드 가독성 | [판정 2026-11-07: 맥에서 확인 필요] GUI 확인 필요 | 앱 실행 후 육안 확인 |
| 캘린더·히스토리 화면, 모드 변경 토스트와 되돌리기 버튼 클릭 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 클릭 단위 UI 테스트 라이브러리 없음, GUI 권한 없음 | 계획 생성→모드 변경→되돌리기 수동 수행 |
| 실제 절전·깨어남 후 계획 재확인 (`pmset sleepnow`) | [판정 2026-11-07: 맥에서 확인 필요] 실기기 절전 필요. 단위 테스트는 가짜 시계·이벤트 | 계획 진행 중 절전 후 깨워 타이머 상태 확인 |
| 로그인 시점 실제 자동 시작 후 계획 자동 개시 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 로그인·재부팅 필요 | 재로그인 후 계획 시각에 잠금 시작 확인 |
| 비상 해제(abort_run) 화면 UX | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] GUI 확인 필요 | 잠금 중 해제 문장 입력 수행 |

### Windows PC 검증 결과 (2026-11-07)

환경: Windows 11 x64, rustc 1.99.1 (x86_64-pc-windows-msvc), node 26.13, pnpm 10.34.9.

| ID | 판정 | 명령 | 결과(종료코드·개수) | 비고 |
|---|---|---|---|---|
| 1-2-CI 원격 CI 실행(본문) | 사람 확인(MANUAL-CHECKLIST) | 해당 없음 | 해당 없음 | 원격 GitHub Actions는 이 PC에서 실행할 수 없다 |
| 1-2-V1 Windows 빌드·테스트 | 검증됨(Windows) | `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (src-tauri); `pnpm tsc --noEmit`, `pnpm lint`, `pnpm test` | 모두 종료 0. `cargo test`: lib 1047 + llm_wire 4 + remote_wire 7 + doc 3 통과, 실패 0(`privacy_schema_guard` 포함); vitest 72 파일 511개 통과 | 이전 결과(SQLite 헤더 부재)는 네이티브 Windows로 대체됨. 수정 커밋 74cab52(.gitattributes, vitest 정적 테스트), cd88e14 |
| 1-2-V2 원격 CI 실행(표) | 사람 확인(MANUAL-CHECKLIST) | 해당 없음 | 해당 없음 | 원격 저장소 푸시 후 두 잡 확인 |
| 1-2-V3 알림 권한·표시 | 맥에서 확인 필요 | 해당 없음 | 해당 없음 | macOS 첫 요청 팝업. Windows 앱 ID 알림은 GUI 육안 확인 필요 |
| 1-2-V4 메뉴바 제목·툴팁·다크모드 | 맥에서 확인 필요 | 해당 없음 | 해당 없음 | 메뉴바는 macOS 전용. Windows 툴팁은 GUI 확인 |
| 1-2-V5 캘린더·히스토리·토스트 클릭 | 사람 확인(MANUAL-CHECKLIST) | 해당 없음 | 해당 없음 | GUI 클릭 필요 |
| 1-2-V6 실제 절전·깨어남 | 맥에서 확인 필요 | 해당 없음 | 해당 없음 | `pmset sleepnow` 실기기 |
| 1-2-V7 로그인 시점 자동 시작 | 사람 확인(MANUAL-CHECKLIST) | 해당 없음 | 해당 없음 | 로그인·재부팅 필요 |
| 1-2-V8 비상 해제 화면 UX | 사람 확인(MANUAL-CHECKLIST) | 해당 없음 | 해당 없음 | GUI 육안 확인 |

## 알려진 위험 / 수용된 잔여 위험

- 시계를 앞으로 점프하면 해당 계획 구간이 지나간 것으로 처리된다(`clock_jump` 읽기 시점 파생으로 기록·노출만).
- 앱이 꺼진 동안의 역방향 시계 변경은 30초 하트비트 `high_water` 정밀도로만 막는다.
- Cmd+Q·Apple Event 종료 우회는 1-1에서 수용한 상태로 1단계-3(3-S5)에서 다룬다.
- LLM 자동 조정은 3단계, 캘린더 가져오기·내보내기는 후속.

## 다음 단위(1단계-3)로 넘기는 항목

- `get_timer_state`/`timer_changed` 계약, `release.rs`(abort_run) 일반화, 요약 `attemptCount: null`(연결 전), 위 검증 안 됨 표.
