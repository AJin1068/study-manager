# 서버 구성 (Supabase)

이 폴더는 2단계 서버 쪽 전부다. 에이전트는 계정이 없어 아래 절차를 실행하지 않았고, SQL은 정적 시험(`pnpm test`)과 Rust FakeBackend로만 확인됐다. 실제 Postgres 적용과 RLS 동작은 **검증 안 됨(V2-8)** 이다.

모델: PC 앱이 계획의 유일한 작성자(로컬 SQLite가 진실)이고, 서버는 폰·알림용 **읽기 미러**다. 서버가 죽거나 낡아도 앱의 타이머·차단·계획은 그대로 동작한다.

## 구성

| 경로 | 내용 |
|---|---|
| `migrations/20260601000000_s2_core.sql` | 테이블 7개, RLS, RPC 3개, 권한. cron 등록은 주석(사람이 실행) |
| `migrations/20260601000100_s2_notify_target.sql` | `notify_channels`에 `utc_offset_min`, `ntfy_server` 추가(알림 시각 표기·ntfy 서버 주소) |
| `functions/missed-plan/index.ts` | Edge Function(Deno). 호출자 확인과 실제 I/O 연결만 한다 |
| `functions/_shared/*.ts` | 순수 TS: `detect.ts`(감지 규칙), `channels.ts`(ntfy·Telegram), `handler.ts`(전체 흐름, I/O 주입), 각 `*.test.ts`(루트 vitest) |
| `../fixtures/notify/contract.json` | ntfy·Telegram 요청 모양. Edge Function 시험과 Rust `NotifyClient` 시험이 공유 |
| `tests/rls_s2.sql` | pgTAP. `supabase test db`(Docker 필요) |
| `tests/schema.static.test.ts` | vitest 정적 검사(RLS·definer·권한·비밀 부재) |
| `../fixtures/sync/rls-matrix.json` | 표×작업 권한 행렬 (SQL·Rust FakeBackend·정적 시험이 공유) |
| `../fixtures/sync/rpc-contract.json` | RPC 이름·인자·반환 코드·허용 역할 (같은 공유 계약) |

이후 단계가 추가한다: 방 스키마(4단계), 짝짓기(5단계).

## 사람이 하는 순서 (한 번)

1. **프로젝트 만들기**: supabase.com에서 새 프로젝트. 지역은 가까운 곳. 프로젝트 URL과 `anon` 공개 키를 적어 둔다.
2. **CLI 연결**: `supabase login` → `supabase link --project-ref <ref>`.
3. **스키마 적용**: `supabase db push`. (`supabase test db`로 `tests/rls_s2.sql`을 먼저 로컬에서 돌려 볼 수 있다. Docker가 필요하다.)
4. **로그인 설정**: Authentication → Providers → Email. 가입 허용, OTP 길이 6, 메일 템플릿에 `{{ .Token }}`(6자리 코드)을 넣는다. 매직 링크는 쓰지 않는다. 기본 SMTP는 발송 한도가 낮으니 쓰기 전에 한도를 확인한다.
5. **앱에 입력**: 앱 설정 → 연동 → 서버 주소와 공개 키. 「연결 시험」으로 도달 여부를 본다.
6. **(2-6 이후) Edge Function과 cron**:
   - `supabase functions deploy missed-plan`
   - 함수 비밀: `supabase secrets set ...` (Telegram 봇 토큰 등. 앱·DB에는 두지 않는다)
   - Vault에 함수 호출 주소와 호출용 토큰을 저장: `select vault.create_secret('<값>', 'missed_plan_url');`, `select vault.create_secret('<값>', 'missed_plan_token');`
   - 마이그레이션 파일 끝의 주석 처리된 `cron.schedule(...)`을 SQL 편집기에서 실행. 이 SQL은 Vault에서 값을 읽을 뿐이며 파일 어디에도 비밀이 없다.

## 권한 행렬

`anon`은 모든 표에서 거부. `owner` = `user_id = auth.uid()`. 클라이언트가 직접 쓰지 못하는 표는 RPC(서버 시각 `now()`)나 서비스 롤로만 바뀐다.

| 표 | select | insert | update | delete | 쓰기 경로 |
|---|---|---|---|---|---|
| devices | owner | owner | owner | owner | 직접 |
| plan_snapshots | owner | - | - | - | `replace_plan_snapshot` |
| plan_occurrences | owner | - | - | - | `replace_plan_snapshot` |
| plan_history_mirror | owner | owner(내 기기만) | - | - | 직접, `(device_id, local_id)` 유일로 멱등 |
| presence | owner | - | - | - | `heartbeat` |
| notify_channels | owner | owner | owner | owner | 직접 (ntfy 토픽은 비밀번호처럼 취급) |
| alert_log | owner | - | - | - | `claim_alert`(서비스 롤 전용) |

이 표는 Rust `FakeBackend`가 코드로 강제하고 정적 시험이 SQL 정책과 대조한다. **RLS 자체의 증명은 아니다.** 실제 증명은 `supabase test db`이며 V2-8로 남아 있다.

## RPC

| 함수 | 호출자 | 반환 코드 |
|---|---|---|
| `replace_plan_snapshot(p_device_id, p_snapshot_hash, p_generated_at_ms, p_horizon_days, p_occurrences)` | authenticated | `ok`, `unchanged`, `device_not_owned`, `invalid_input` |
| `heartbeat(p_device_id, p_app_version, p_studying)` | authenticated | `ok`, `device_not_owned` |
| `claim_alert(p_user_id, p_occurrence_key, p_kind)` | service_role만 | `claimed`, `duplicate` |

규칙: 모든 definer 함수는 `set search_path = public, pg_temp`, `revoke execute ... from public, anon`, 필요한 롤에만 `grant`. 소유 검사는 함수 안에서 `auth.uid()`로 한다. **실패는 RAISE가 아니라 반환 코드로 알린다.** RAISE는 트랜잭션을 되돌려 실패 시도의 기록(이후 4단계의 입장 시도 횟수 등)까지 지우기 때문이다.

## 서버에 두지 않는 것

창 제목, 전체 URL, 입력한 문장, 집중·차단 이벤트, 탭 기록은 컬럼 자체가 없다. 계획 이름·과목·시각·반복 전개만 올라간다(AGENTS.md §6, `docs/egress-policy.md`).

## 비밀 취급

- 앱에는 공개 `anon` 키만 입력한다. `service_role` 키를 앱·저장소·fixture에 넣지 않는다(정적 시험이 JWT 모양과 서비스 롤 키 변수 이름을 검사한다).
- Edge Function의 비밀은 Supabase 함수 비밀 또는 Vault에만 둔다.

## 놓친 계획 알림 배포 (사람이 하는 순서, V2-7)

에이전트는 계정이 없어 배포·cron·실제 수신을 실행하지 않았다. 로직은 `pnpm test`(감지 골든, 5분 산수 시뮬레이션, 모의 채널 전체 흐름)로만 확인됐다.

1. **함수 비밀 설정**: `supabase secrets set MISSED_PLAN_DB_KEY=<서버 키> MISSED_PLAN_TOKEN=<임의 긴 문자열> TELEGRAM_BOT_TOKEN=<BotFather 토큰>`. Telegram을 안 쓰면 마지막은 생략. `NOTIFY_MODE=mock`은 운영에 설정하지 않는다. `SUPABASE_URL`, `SUPABASE_ANON_KEY`는 플랫폼이 넣는다.
2. **배포**: `supabase functions deploy missed-plan --no-verify-jwt` (cron은 사용자 JWT가 없고 `MISSED_PLAN_TOKEN`으로 직접 확인한다. 사용자 시험 호출은 함수 안에서 JWT를 검증한다).
3. **Vault에 값 저장**: SQL 편집기에서 `select vault.create_secret('<함수 URL>', 'missed_plan_url');`와 `select vault.create_secret('<MISSED_PLAN_TOKEN과 같은 값>', 'missed_plan_token');`.
4. **cron 등록**: `20260601000000_s2_core.sql` 끝의 주석 처리된 `cron.schedule('missed-plan-check', '* * * * *', ...)`를 SQL 편집기에서 한 번 실행. 확장 `pg_cron`, `pg_net`, `supabase_vault`가 켜져 있어야 한다.
5. **확인**: 앱 설정 → 연동 → 폰 알림에서 「테스트 알림 보내기」. 그다음 계획 시작 시각에 PC를 끄고 폰 도착 시각을 잰다(V2-1).

동작 규칙: 시작 + 유예(1~3분) 이후 30분 안에, 시작 이후 신호가 없고 상태가 `planned`인 발생만 알린다. 같은 발생은 `claim_alert`(서비스 롤 전용, `alert_log` 유일 제약)로 한 번만 보낸다. 먼저 claim하고 나중에 보내므로 전송이 두 번 실패하면 그 발생의 알림은 다시 시도하지 않는다(응답의 `failed`로 보인다).
