# 2단계 검증 기록

2단계(서버 연결과 놓친 계획 알림) 전체의 검증 문서다. 에이전트는 Supabase 계정, 실제 ntfy·Telegram, 실제 기기, GUI가 없으므로 아래 「검증 안 됨」 항목은 실행하지 않았다. 2단계 완료 기준(PC를 안 켜면 5분 안에 폰으로 알림이 온다)은 V2-1을 사람이 확인하기 전까지 충족된 것으로 표기하지 않는다.

## 스토리 상태

| 스토리 | 내용 | 코드 상태 | 자동 증명 |
|---|---|---|---|
| S2.1 | 외부 포트·모드·비밀·HostPolicy·반출 페이로드 | 구현 | 단위·와이어 시험, `egress_payload_guard` |
| S2.2 | 서버 SQL(스키마·RLS·RPC)과 FakeBackend | 구현 | 정적 시험, FakeBackend 권한 행렬 |
| S2.3 | Supabase 로그인·기기 등록 | 구현 | 가짜 서버·루프백 와이어 시험 |
| S2.4 | 계획·히스토리 미러(발생 전개, 해시, 아웃박스 커서) | 구현 | `sync::snapshot`, `sync::push`, `sync::worker` 시험 |
| S2.5 | 켜짐 신호(60초, 재시도 5·10·20·30초, 수면·깨어남) | 구현 | `sync::presence` 가짜 단조 시계 시험 |
| S2.6 | 놓친 계획 감지·알림(Edge Function, `NotifyClient`) | 구현 | `supabase/functions/_shared/*.test.ts`, `remote::notify` |
| S2.7 | 폰 알림 설정 UI, QR | 구현 | `notifyView.test.ts`, `remote::qr` |
| S2.8 | ICS 가져오기·내보내기 | 구현 | `plan_ics`, `plan_import` 시험 |

## 게이트 명령과 결과

| 명령 | 위치 | 결과 |
|---|---|---|
| `cargo fmt --check` | `src-tauri` | 종료 0 (Windows 11 x64, rustc 1.99.1). 이전 결과: 실행 불가, rustfmt 미설치. cd88e14로 `cargo fmt` 일괄 적용 후 통과 |
| `cargo clippy --all-targets -- -D warnings` | `src-tauri` | 종료 0 (Windows 네이티브 재실행. a22bb03으로 `browser_ipc.rs` dead_code 수정 후) |
| `cargo test` | `src-tauri` | Windows 네이티브: lib 1047 + `llm_wire` 4 + `remote_wire` 7 + doc 3 통과, 실패 0. 이전 결과: 515 + 통합 7 + doc 3. `cfg(unix)` 시험은 Windows에서 컴파일되지 않음 |
| `pnpm tsc --noEmit` | 루트 | 종료 0 |
| `pnpm lint` | 루트 | 종료 0 |
| `pnpm test` (supabase 시험 포함) | 루트 | 72파일 511개 통과, 종료 0. 이전 결과: 244개. 74cab52(`.gitattributes` eol=lf) 후 통과 |
| `supabase test db` (Docker 필요) | 루트 | 실제 CLI는 여전히 실행 안 함(이 PC에 Docker·WSL2·Supabase CLI 없음). 대체: 스텁 PostgreSQL 16.4에서 `supabase/tests/stub/run.ps1`로 마이그레이션 8개 적용(오류 없음) 후 `rls_s2` 18/18 등 통과(아래 V2-8). 스텁이라 부분 검증. 이전 결과: pgTAP 검증 안 됨 |
| Windows 타깃 `cargo check --target x86_64-pc-windows-msvc` | `src-tauri` | 이 PC가 Windows(x86_64-pc-windows-msvc) 네이티브라 위 `cargo clippy`·`cargo test`·`pnpm tauri build --bundles nsis`(종료 0)가 대체한다. 이전 결과: 종료 101, 번들 SQLite C 헤더 부재 |

## 에이전트가 자동으로 증명한 것

- 계획 발생 전개(오늘-1일 ~ +14일), 건너뜀·취소·이동 상태, 해시가 생성 시각과 무관하고 내용·시간대에 따라 바뀜.
- 히스토리 1만 행이 500건 단위로 중복 없이 올라가고, 중간 실패 후 마지막 성공 배치 뒤에서 재개. 요청 중에는 DB 잠금을 잡지 않음.
- 건너뛰기가 3초 안에 서버 미러에 반영(가짜 시계). 서버가 기기를 잊으면 재등록하고 전부 다시 보냄. 로그아웃·재로그인 시 커서 초기화.
- 켜짐 신호: 정상 60초, 수면 중 0건, 깨어난 직후 즉시 1건, 실패 시 5·10·20·30초 재시도, 3회 연속 실패 시 `presence_unreachable`, 로그아웃·미설정에서 전송 0, 시계 역행에도 폭주 없음.
- 감지 규칙 골든(정상·미가동·시작 후 가동·이미 알림·건너뜀·지난 계획·연속 두 계획·유예 경계 ±1초)과 유예 4 이상 거부(설정 검증기, 서버에서는 3으로 보수 처리).
- 5분 산수: 모든 cron 위상(5초 간격)과 유예 1·2·3분에서 알림이 정확히 1건, 4분 10초 이하로 도착. 같은 발생은 두 번째 틱에서 `claim_alert` 중복으로 0건.
- 문구는 시각과 고정 문장뿐이며 과목·계획 이름·토픽이 들어갈 입력 경로가 없음. 데스크톱 `NotifyClient`와 Edge Function이 `fixtures/notify/contract.json` 하나로 같은 요청 모양을 시험.
- ntfy 서버 주소 검사(http, localhost, IP, 내부 이름, 포트, 경로, 인증 정보 거부).
- 반출 가드: 새 페이로드 표본이 금지 키·카나리 문자열을 포함하지 않음. `privacy_schema_guard` 통과(m009 이후 새 로컬 마이그레이션 없음).
- QR: `ntfy://<호스트>/<토픽>` 링크가 정사각 행렬로 인코딩되고 SVG로 그려짐(결정적). **스캔 왕복(디코딩)은 디코더 의존성이 없어 시험하지 않았다. V2-10.**

## 검증 안 됨

| ID | 스토리 | 항목 | 안 된 이유 | 사람이 하는 한 줄 절차 |
|---|---|---|---|---|
| V2-1 | S2.6, S2.5 | 실제 ntfy·Telegram 도착과 「5분 안」 실측, 켜짐 신호 p95 도착 지연 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실계정·실제 PC 종료·실제 폰 필요 | `supabase/README.md` 배포 절차를 마친 뒤 계획 시작 시각에 PC를 끄고 폰 알림 도착 시각을 재서 시작+5분 안인지 3회 확인 |
| V2-2 | S2.1 | 실제 키체인 동작(macOS 팝업, Windows Credential Manager) | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] OS 키체인·GUI 필요, 서명 없는 개발 빌드는 팝업이 뜸 | 앱에서 키를 저장하고 재시작해 「저장됨」으로 남는지, 키체인 접근에 `study-manager` 항목이 생겼는지 확인 |
| V2-3 | S2.1, S2.7 | 연동 화면과 폰 알림 패널의 실제 렌더·레이아웃 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] GUI 확인 필요 | 설정 → 연동에서 채널 선택·QR·유예·상태 표시가 라이트·다크에서 겹치지 않는지 눈으로 확인 |
| V2-4 | S2.2, S2.4 | 실제 Supabase RPC 왕복(`replace_plan_snapshot`, `heartbeat`, 히스토리 insert) | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실제 프로젝트 필요 | 로그인한 앱에서 계획을 만들고 Supabase 표 편집기에서 `plan_occurrences`·`presence`·`plan_history_mirror` 행이 생기는지 확인 |
| V2-5 | S2.3 | 실제 OTP 메일 수신과 로그인 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실제 메일·프로젝트 필요 | 앱에서 코드 받기 → 메일의 6자리 코드로 로그인되고 「로그인됨」이 뜨는지 확인 |
| V2-6 | S2.5 | 실제 수면/깨어남에서 신호가 멈추고 다시 시작함 | [판정 2026-11-07: 맥에서 확인 필요] 실기기 필요 | 맥북 덮개를 1분 닫았다 열고, 열린 뒤 약 2초 안에 서버 `presence.last_seen_at`이 갱신되는지 확인 |
| V2-7 | S2.6 | Edge Function 배포와 `pg_cron` 1분 실행, Vault·`pg_net` 가용성 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 계정 필요, 무료 플랜 기능 가정 | `supabase/README.md`의 배포 절차를 실행한 뒤 함수 로그에 1분마다 호출이 찍히고 「테스트 알림 보내기」가 도착하는지 확인 |
| V2-8 | S2.2 | 실제 Postgres에서 마이그레이션 적용과 RLS 동작 | [판정 2026-11-07: 부분] Docker·계정 필요 | `supabase test db`를 실행해 `tests/rls_s2.sql`이 모두 통과하고 `supabase db push`가 오류 없이 끝나는지 확인 |
| V2-9 | S2.8 | 실제 Google/Apple 캘린더가 내보낸 .ics 호환 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실제 내보내기 파일 필요 | 구글·애플 캘린더에서 내보낸 .ics를 가져오기 미리보기로 열어 일정·반복·「건너뜀 N건」이 맞는지 확인 |
| V2-10 | S2.7 | QR을 폰 카메라로 스캔해 ntfy 앱이 구독을 여는지 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실제 폰·ntfy 앱 필요 | 토픽 만들기 후 QR을 ntfy 앱으로 스캔해 같은 토픽이 구독되고 테스트 알림이 도착하는지 확인 |
| V2-11 | S2.6 | 서버가 PC의 켜짐을 오판하는 경우(네트워크만 끊김) 문구 체감 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실사용 필요 | 계획 시각에 와이파이만 끄고 알림 문구 「신호가 없어요」가 납득되는지 확인 |
| V2-12 | S2.4 | 실제 시간대·서머타임 경계에서 발생 시각과 알림 시각표기 | [판정 2026-11-07: 맥에서 확인 필요] 실기기 시간대 변경 필요 | 맥의 시간대를 바꾼 뒤 서버 `plan_occurrences.start_at_utc_ms`와 알림의 시각이 계획 시각과 같은지 확인 |

### Windows PC 검증 결과 (2026-11-07)

환경: Windows 11 x64, rustc 1.99.1. 스텁 PostgreSQL 16.4는 실제 Supabase가 아니다(GoTrue·PostgREST·Realtime·Vault·pg_cron 없음, pgTAP 대신 pgtap-lite).

| ID | 판정 | 명령 | 결과(종료코드·개수) | 비고 |
|---|---|---|---|---|
| V2-1 | 사람 확인(MANUAL-CHECKLIST) | 없음 | 실행 안 함 | 실계정·실제 PC 종료·실제 폰 필요 |
| V2-2 | 사람 확인(MANUAL-CHECKLIST) | 없음 | 실행 안 함 | Windows Credential Manager와 macOS 팝업 모두 GUI 필요. 맥 쪽은 맥에서도 확인 |
| V2-3 | 사람 확인(MANUAL-CHECKLIST) | `pnpm build:web`, `pnpm test` | 종료 0, 72파일 511개 통과 | 정적·단위 시험만. 실제 렌더·레이아웃은 GUI를 보지 않았다 |
| V2-4 | 사람 확인(MANUAL-CHECKLIST) | 없음 | 실행 안 함 | 실제 Supabase 프로젝트 필요 |
| V2-5 | 사람 확인(MANUAL-CHECKLIST) | 없음 | 실행 안 함 | 실제 메일·프로젝트 필요 |
| V2-6 | 맥에서 확인 필요 | 없음 | 실행 안 함 | 맥북 덮개 수면·깨어남 필요 |
| V2-7 | 사람 확인(MANUAL-CHECKLIST) | 없음 | 실행 안 함 | 실제 계정·Edge Function 배포·pg_cron 필요 |
| V2-8 | 부분 | `supabase/tests/stub/run.ps1` (스텁 PG 16.4, 마이그레이션 8개 적용 후 `rls_*.sql` 7개 실행) | 마이그레이션 오류 없음. `rls_s2` 18/18, `rls_s4` 26/26, `rls_s4_profile` 27/27, `rls_s4_shared_wiki` 22/22, `rls_s5_anonymous_guard` 23/23, `rls_s5_notify_phone_app` 4/4, `rls_s5_phone` 52/52 (합계 172). 음성 대조(anon에 devices select 부여)는 의도대로 1개 단언 실패 | 스텁이라 부분. 실제 `supabase test db`·`supabase db push`는 Docker·WSL2·CLI 없어 미실행 |
| V2-9 | 사람 확인(MANUAL-CHECKLIST) | 없음 | 실행 안 함 | 실제 Google/Apple 내보내기 파일 필요 |
| V2-10 | 사람 확인(MANUAL-CHECKLIST) | 없음 | 실행 안 함 | 실제 폰·ntfy 앱 필요 |
| V2-11 | 사람 확인(MANUAL-CHECKLIST) | 없음 | 실행 안 함 | 실사용 체감 판단 필요 |
| V2-12 | 맥에서 확인 필요 | 없음 | 실행 안 함 | 맥의 시간대 변경 필요 |

## 알려진 잔여 위험 (수용·추적)

- 알림은 먼저 `claim_alert`하고 나중에 보낸다. 전송이 두 번 실패하면 그 발생은 다시 시도하지 않는다(중복 알림보다 누락을 택한 것이며 함수 응답의 `failed`로 보인다).
- 시작 전부터 켜져 있다가 중간에 꺼진 경우는 감지 범위 밖이다(시작 이후 신호가 이미 있음).
- PC가 켜져 있고 네트워크만 끊긴 경우 오탐이 날 수 있다.
- 미러 워커는 2초마다 변경 표식을 확인하는 방식이라 명령마다 훅을 걸지 않는다. 표식이 같은 값으로 되돌아가는 변경은 5분 전체 재전송이 잡는다.
- 실제 Telegram 봇 토큰은 Edge Function 비밀이라 데스크톱 `NotifyClient`는 Telegram 시험을 함수에 위임한다. 함수가 배포되기 전에는 「테스트 알림」이 `http_error`로 실패한다.
