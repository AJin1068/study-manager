# AGENTS.md — study-manager

이 문서는 이 저장소에서 작업하는 모든 에이전트와 개발자가 따라야 하는 규칙이다. 프로젝트 구조가 바뀌면 이 문서도 같은 변경에서 갱신한다.

## 1. 프로젝트 개요

- 목적: 공부 시간 관리 데스크톱 앱. 공부 중 방해 요소(앱, 탭)를 관리하고 기록한다.
- 현재 단계: **5단계(폰 앱) 부분 구현: PC 쪽(link-protocol·짝짓기·임대 발행·차단기 포트·푸시·APK 카드)과 `phone/`(폰 앱 본체: `phone/core` 호스트 시험 통과, `phone/src-tauri` Tauri 모바일 셸, `phone/src` 화면 4개, Kotlin 차단 플러그인)이 작성되었으나, 안드로이드용으로 빌드·실행된 적이 없고(Kotlin 포함) 포그라운드 서비스·폰 쪽 계획 수신 경로·화면 켜짐 계수는 없다. 4단계(스터디방)는 구현 완료(Realtime 소켓 전송 미구현, 그림은 자리표시), 사람 검증 대기.** 1단계 완료 기준(일주일 실사용), 2단계(실제 서버에서 5분 안 알림), 3단계(한 과목의 위키가 실제 공부로 자라나고 퀴즈가 나옴), 4단계(친구 2명 이상이 같은 방에서 공부함; 실제 Postgres RLS 포함), 5단계(공부 시간에 폰이 PC와 함께 잠김; 실기기)는 사람이 확인해야 하며, 확인 전까지 단계 완료로 표기하지 않는다. 검증 안 된 항목은 `docs/stage-2-verification.md`, `docs/stage-3-verification.md`, `docs/stage-4-verification.md`, `docs/stage-5-verification.md`.
- 모듈 구조: `src-tauri/src/` — `commands/`(`mod.rs`: AppState·종료 게이트, `settings`·`study`·`downtime`·`power`: Tauri command), `db/`(`mod.rs`, `migrations/mNNN_*.sql`), `store/`(SQL 접근, `remote.rs`는 반출 기록·동기화 커서), `remote/`(외부 포트: 포트·모드, HTTP 계층·호스트 정책, 봉인된 반출 페이로드, 비밀, Supabase·알림 어댑터, 가짜 서버), `sync/`(미러 워커, 켜짐 신호), `llm/`(LLM 포트·모의·Anthropic 어댑터·예산·봉인 프롬프트), `wiki/`(위키 저장소·넣기·묻기·회고·점검·주제 추천), `quiz/`(문제은행, 조기 해제 관문, 복습 카드), `chat/`(계획 챗봇), `messages/`(응원·채찍), `rooms/`(스터디방 포트·폴링·랭킹·Realtime 코덱), `character/`(캐릭터 조합·자리표시 SVG 렌더, 데이터 `src-tauri/assets/character/`), `shop/`(상점), `profile/`(프로필 홈·방명록·선물), `shared_wiki/`(공유 위키), `phone/`(폰 짝짓기·임대 발행·차단기 포트·모의 차단기·APK 카드, 판정 로직은 `crates/link-protocol`), `study_context.rs`, `plan_adjust.rs`, `growth.rs`, `plan_ics.rs`·`plan_import.rs`(ICS), `settings.rs`, `study.rs`(잠금·공부 모드), `downtime.rs`, `power.rs`, `platform/`, `tray.rs`, `clock.rs`, `error.rs`, 루트의 `crates/link-protocol/`(독립 크레이트: 자체 Cargo.toml·lock, 루트 Cargo 워크스페이스 없음, `src-tauri`가 경로 의존으로만 사용, 임대 상태기계·직렬화 타입·폰 쪽도 공용), 루트의 `phone/`(폰 앱 독립 프로젝트: 지금은 `phone/plugins/blocker/android/` Kotlin 차단 플러그인 소스뿐이며 `src-tauri`에 의존하지 않음, 아이폰은 Swift 없이 항상 Unsupported), 루트의 `supabase/`(마이그레이션·pgTAP·Edge Function `functions/`과 순수 로직 `functions/_shared`)와 `fixtures/`(Rust·TS 공유 계약). 웹 보기는 루트 `web/`(+ `vite.web.config.ts`, `pnpm build:web`). UI는 `src/` — `api/`(`index.ts`가 `settings`·`study`·`downtime` 재수출), `hooks/`, `pages/`. CI: `.github/workflows/ci.yml`(macOS·Windows, link-protocol 호스트 시험 포함), `.github/workflows/android.yml`(APK 빌드·릴리스, 실행된 적 없음).

## 1-1. 요구사항 기준 (가장 먼저 읽을 것)

- **`docs/PLAN.md`가 모든 요구사항의 단일 기준(SSOT)이다.** 기능은 번호(1~17), 작업은 "N단계-M"으로 부른다.
- 작업을 시작하기 전에 PLAN.md에서 해당 기능 번호, 로드맵 단계, "설계 보완 사항"을 먼저 읽는다.
- PLAN.md에 이미 정해진 내용은 사용자에게 다시 묻지 않는다. 비어 있거나 서로 충돌하는 부분만 묻는다.
- 범위를 임의로 줄이지 않는다. 사용자는 기능 1~17 전체 구현을 원한다. 지금 단계에서 안 하는 기능은 "N단계에서 구현"으로 남긴다.
- 요구사항이 바뀌면 PLAN.md를 같은 변경에서 갱신한다. 이 AGENTS.md와 PLAN.md가 충돌하면 사용자에게 확인한다.

## 2. 기술 스택

| 영역 | 기술 |
|---|---|
| 데스크톱 셸 | Tauri 2 |
| 코어 로직 | Rust |
| UI | React + TypeScript |
| 로컬 저장소 | SQLite |
| 서버 | Supabase (2단계 도입 완료: 읽기용 미러 모델, 계정 없이도 모의 모드로 동작) |
| 브라우저 차단 | 크롬 확장 프로그램 (1단계-4) |
| 앱 내 LLM | Anthropic API, Claude Haiku (앱 설정의 사용자 API 키, 3단계 도입 완료: 기본 꺼짐, 키 없이도 모의·로컬 경로로 동작) |

## 3. 아키텍처 원칙

- **Rust 코어가 소유하는 것**: OS 연동, 프로세스/창 조회, 앱 종료, SQLite 접근, 기록 수집.
- **UI(React)가 소유하는 것**: 화면, 상태 표시, 사용자 입력. OS API나 DB를 직접 호출하지 않는다.
- UI ↔ 코어 통신은 Tauri command와 event로만 한다. command 인자/반환 타입은 명시적으로 정의한다.
- 저장소 접근은 repository 계층 뒤에 둔다. 나중에 Supabase를 붙일 때 호출부를 바꾸지 않도록 SQLite 구현을 교체 가능한 형태로 만든다.
- **외부 포트 규칙 (`remote/`, 3단계 `llm/`):** 외부로 나가는 모든 호출은 포트 뒤에 두고 모의(`FakeBackend`/기록 어댑터)와 실제 어댑터를 둘 다 만든다. 요청 본문은 봉인된 `EgressPayload` 타입으로만 만들고(허용 목록은 docs/egress-policy.md), 전송은 주입된 `HostPolicy`가 호스트를 제한한다. 실제 어댑터는 실제 호스트나 자격증명 없이 가짜 전송·루프백 서버로만 시험한다. 서버 쪽 로직은 순수 TS(`supabase/functions/_shared`)로 두고 vitest로 시험한다.
- 필요 이상의 추상화, 미사용 설정, 호환성 계층을 만들지 않는다. 가장 단순하게 요구사항을 충족하는 구현을 택한다.

## 4. 크로스 플랫폼 규칙 (macOS / Windows)

- **macOS와 Windows 모두 빌드되어야 한다.** 한 쪽에서만 빌드되는 변경은 완료로 보지 않는다.
- **OS별 코드는 한 모듈에 모은다.** 예: `src-tauri/src/platform/` 아래에 공통 인터페이스(`mod.rs`), `macos.rs`, `windows.rs`를 둔다.
  - `#[cfg(target_os = ...)]` 분기는 이 모듈 밖에 흩어 두지 않는다.
  - 다른 모듈은 플랫폼 중립 인터페이스만 호출한다.
  - 한 OS에서만 지원되는 기능은 인터페이스에서 명시적으로 "미지원" 결과를 반환하고, 조용히 무시하지 않는다.
- OS API 사용 시 권한(macOS 접근성/자동화 권한 등)과 Windows 권한 차이를 해당 모듈 주석이나 문서에 기록한다.
- 경로는 `std::path`/Tauri path API를 사용하고 `/`나 `\`를 하드코딩하지 않는다.

## 5. 앱 종료 정책

- **시험 모드가 기본값이다.** 시험 모드에서는 종료 대상을 판단하고 로그만 남긴다. 실제 종료 동작은 하지 않는다.
- 실제 종료는 사용자가 명시적으로 켰을 때만 수행한다. 설정 기본값을 실행 모드로 두지 않는다.
- 종료 순서:
  1. **정상 종료를 먼저 시도한다** (macOS: `terminate`/Apple Event 기반 종료, Windows: `WM_CLOSE`/창 닫기 요청).
  2. 제한 시간 내 종료되지 않은 경우에만 강제 종료를 시도한다 (macOS: `SIGKILL`, Windows: `TerminateProcess`).
  3. 정상 종료 대기 시간과 강제 종료 여부는 설정으로 노출하고, 각 시도 결과를 로그에 남긴다.
- 저장되지 않은 작업을 날릴 수 있으므로, 강제 종료는 정상 종료 실패 후의 최후 수단으로만 쓴다.
- 시스템 프로세스, 보안 프로세스, 자기 자신(study-manager)은 종료 대상에서 제외한다.

## 6. 개인정보 및 데이터 저장

- **탭 기록과 창 제목은 로컬에만 저장한다.** SQLite 로컬 파일 외 어디에도 보내지 않는다.
- 서버(Supabase)에는 "공부 중/딴짓" 상태와 횟수, 계획, 사용자가 공유하기로 한 위키만 보낸다. 2단계 현재 허용 목록: 로그인 정보, 기기 등록, 켜짐 신호(기기 ID·앱 버전·공부 잠금 불리언), 계획 발생 목록과 계획 수정 히스토리, 알림 채널 설정. 전체 목록은 `docs/egress-policy.md`.
- 알림 문구에는 과목·계획 이름을 넣지 않는다(잠금 화면 노출). 시각과 고정 문장만 쓴다.
- 비밀(로그인 토큰, API 키, ntfy 토픽)은 OS 키체인에만 저장한다. 설정·`setting_history`·로그·DB에는 값도 해시도 남기지 않고 설정 여부만 남긴다. 서버에는 비밀을 두지 않는다(Telegram 봇 토큰은 Edge Function 비밀). 코드·fixture에 service role 키나 JWT 모양 문자열을 넣지 않는다.
- 폰(5단계): 폰은 임대 수신 확인(seq·잠금 불리언·서비스 켜짐)과 구간별 집어 든 횟수(구간 ID+횟수)만 서버에 보낸다. 앱 이름·패키지명·사용 내역은 payload에 필드가 없고 허용 앱 목록은 폰 로컬에만 있다. 짝짓기 QR에는 5분 1회용 코드·서버 주소·공개 키만 넣는다(장기 비밀 금지). 폰 임대는 만료되면 반드시 풀린다(영원히 잠기지 않음).
- 새 반출은 `docs/egress-policy.md`와 `remote::payload` 타입, `egress_payload_guard` 표본을 같은 변경에서 추가한다. 프라이버시를 줄이는 쪽 설정 변경(모의 전환, 알림 끄기, 비밀 삭제, 로그아웃)은 공부 run 중에도 허용하고 `applied_privacy_tighten`으로 기록한다.
- 브라우저 탭 판단은 로컬에서 한다: 규칙 → 캐시 → 애매할 때만 Haiku. 기본 꺼짐(`llm.judge_enabled`, 키와 동의 전 전송 0). 보내는 것은 도메인·줄인 탭 제목(120자 이하)·현재 주제뿐이며 경로·쿼리·전체 URL은 보내지 않고 저장하지 않는다(캐시에는 해시와 도메인만). 창 제목 LLM 분류는 호출부를 만들지 않았고 기본 꺼짐이다.
- 위키·챗봇·인사이트 등 LLM 반출은 `docs/egress-policy.md`의 LLM 행과 `llm::egress` 타입으로만 한다. 공부 맥락(`context.*`, 기본 꺼짐)의 라벨은 80자 이하 요약만 로컬에 30일 보관하고 별도 동의(`context.llm_consent`) 없이는 LLM에 보내지 않는다.
- 탭 기록·창 제목을 Supabase 등 외부 서버로 동기화하는 기능은 만들지 않는다. 향후 동기화가 필요하면 별도 결정 문서를 먼저 작성하고 사용자 승인을 받는다.
- 텔레메트리, 원격 크래시 리포트, 분석 SDK에 창 제목이나 URL을 포함하지 않는다.
- 로그에도 창 제목과 전체 URL을 기록하지 않는다. 필요하면 도메인 수준이나 해시로 줄인다.
- 로컬 DB 파일 경로와 보관 기간은 설정으로 관리하고, 사용자가 삭제할 수 있는 경로를 제공한다.

## 7. 코딩 규칙

- Rust: 에러는 `Result`로 전파하고 `unwrap`/`expect`는 테스트 코드 외에서 쓰지 않는다. 플랫폼 에러는 원인이 보이도록 감싼다.
- TypeScript: `strict` 모드를 유지하고 `any`를 쓰지 않는다. Tauri command 타입은 Rust 쪽 정의와 맞춘다.
- 로그는 구조화해서 남기고, 시험 모드의 "would terminate" 결정도 같은 형식으로 남긴다.
- 주석은 "왜"를 설명할 때만 쓴다.
- 기존 패턴이 있으면 따른다. 병렬 컨벤션을 만들지 않는다.

## 8. 검증

- 모든 변경은 관련 테스트 또는 실행 시나리오로 검증한다.
- 최소 검증 항목:
  - Rust: `cargo check` 및 `cargo test`가 macOS와 Windows 타깃 모두에서 통과해야 한다.
  - UI: 타입 체크와 린트 통과.
  - 종료 로직: 시험 모드에서 실제 프로세스가 종료되지 않는지, 실행 모드에서 정상 종료 → 강제 종료 순서가 지켜지는지 확인.
  - 개인정보: 창 제목/URL이 로그와 외부 전송 경로에 나오지 않는지 확인.
- CI에 macOS와 Windows 빌드 매트릭스를 두는 것을 목표로 한다. 설정 전까지는 두 OS에서 수동 빌드 결과를 보고한다.
- 검증하지 않은 항목은 "검증 안 됨"이라고 명시한다. 단계 문서(`docs/stage-N-verification.md`)에 `V{N}-{k} | 스토리 | 항목 | 안 된 이유 | 사람이 하는 한 줄 절차` 표로 남긴다.
- 2~4단계 게이트 명령: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `pnpm tsc --noEmit`, `pnpm lint`, `pnpm test`(루트 vitest가 `supabase/**`·`web/**` 시험 포함), 웹 변경 시 `pnpm build:web`. 실제 Postgres가 필요한 `supabase test db`는 Docker가 있을 때만 돌리고 없으면 V2-8로 남긴다. 5단계는 추가로 `crates/link-protocol`에서 `cargo test`·`cargo clippy --all-targets -- -D warnings`를 돌린다. Kotlin·Android 빌드는 환경이 없으면 `docs/stage-5-verification.md`의 V5-x로 남긴다.

## 9. 문서와 파일

- 설계 결정은 `docs/`에 짧게 기록한다. 요청 없이 새 문서를 대량으로 만들지 않는다.
- 새 파일보다 기존 파일 수정을 우선한다.
- 이 AGENTS.md는 규칙 변경 시 같은 커밋에서 함께 갱신한다.
- 용어: 잠금 상태는 `lock_held`(Rust `study::lock_held`, `LockState`, TS `lockHeld`)이다. DB 컬럼 `study_state.studying`은 이력상 이름이며 같은 의미다.
- 새 마이그레이션은 마지막 번호 뒤에 붙인다(`db/migrations/mNNN_*.sql`, 서버는 `supabase/migrations/<타임스탬프>_*.sql`). 이미 적용된 파일은 고치지 않는다.
- 퀴즈 관문(3단계, O-8·O-26): 위키 퀴즈 증명은 `PassKind::EarlyRelease`에만 쓴다(문제 3개 전부 정답, 하루 4회, 쿨다운 3분). 시도를 소진하면 그날 조기 해제는 타이핑으로도 열리지 않는다. 긴급 패스·오늘 건너뛰기는 항상 타이핑과 각자의 일일 한도를 유지한다. 딴짓 복습 카드는 차단을 풀지 않는다. 위키 문제가 3개 미만이면 조기 해제도 타이핑이다.
- LLM(3단계): 호출은 `llm::service::LlmService::complete`로만 한다(꺼짐·키 없음·일일 예산 게이트, `llm_usage` 기록). 요청 본문은 `llm::egress::Prompt`/`MessagesRequest`뿐이고, 위키 파일은 미리보기 확인 뒤 `wiki::service::apply_ingest`에서만 쓴다. LLM이 꺼져 있어도 위키의 로컬 경로(제목 단위 정리·검색·찾은 페이지 보여주기)는 동작해야 한다. 근거: `docs/decision-llm-egress.md`.
- 폰(5단계): iOS는 `ScreenTimeBlocker`가 항상 `Unsupported`를 돌려준다(조용한 성공 금지, §4). Swift 파일을 만들지 않는다. 폰 앱(`phone/core`, `phone/src-tauri`)은 `link-protocol`과 `fixtures/phone/`만 공유하고 `src-tauri`를 의존하지 않는다(objc2·keyring이 Android 타깃에서 깨짐). 그래서 `phone/core`의 `allow_list`·`blocker*`·`android_blocker`는 `src-tauri/src/phone/`의 같은 이름 파일의 복사본이다: 한쪽을 고치면 다른 쪽도 고친다. 폰 쪽 시험은 `cd phone/core && cargo test`, `cd phone/src-tauri && cargo test`(루트 워크스페이스 없음). 결정 근거 `docs/decision-phone-architecture.md`.
- 개인정보 스키마 가드: 테이블·컬럼 이름에 `title`, `window_title`, `url`, `typed*`, `sentence*`를 쓰지 않는다. `db` 테스트 `privacy_schema_guard`가 `sqlite_master`로 검사하며 모든 마이그레이션이 통과해야 한다. 로그 검증은 `testutil::capture_logs`를 쓴다.

## 10. 열린 질문 (결정 전까지 보류)

- 시험 모드 로그의 보관 위치와 보관 기간
- 정상 종료 대기 시간 기본값

결정됨:
- 종료 대상 선정 기준: 블랙리스트(카테고리 프리셋, PLAN.md 2번). 브라우저는 엄격 모드(허용 목록) 선택 가능.
- Supabase 동기화 범위: 상태·횟수·계획·공유 위키만. 탭 기록과 창 제목은 제외 (6절).
- 잠금 범위: 계획된 공부 run 전체(공부+휴식) 동안 전체 설정·종료·계획 변경 잠금. run 밖에서만 변경 (PLAN.md 13, 17번, D-1).
- 확장 ↔ 로컬 앱 통신 방식: native messaging + 로컬 IPC (docs/stage-1-4-transport-decision.md).
- 동기화 모델(2단계): 쓰기는 PC 한 곳, 로컬 SQLite가 진실, Supabase는 읽기용 미러. 서버가 죽어도 로컬 기능은 약해지지 않는다.
- 놓친 계획 알림(2단계): 유예 기본 3분(최대 3), 서버 감지 1분 주기, 문구 무과목. Telegram 봇은 공유 봇 1개(토큰은 Edge 비밀), 기본 채널 ntfy.
- LLM 3단계: 키·동의 전 전송 0(`llm.enabled`, `llm.judge_enabled` 기본 꺼짐; PLAN 13번의 「LLM 판단 켜기: 켜짐」에서 의도적으로 변경). 위키 퀴즈는 조기 해제에만 관문이며 시도 소진 시 조기 해제는 타이핑으로 열리지 않는다(`docs/stage-3-verification.md` 잔여 위험). 근거 `docs/decision-llm-egress.md`.
- 스터디방(4단계): 친구 = 같은 방 멤버. 서버 쓰기는 전부 `security definer` RPC(반환 코드 `{"code": ...}`), 방 화면은 `room_snapshot`/`list_my_rooms`만 읽는다. 기본 공개는 레벨·상태뿐이고 켜는 방향은 run 중 잠긴다. 공유 위키는 공유 표시+확인 화면을 거친 페이지만, LLM 합치기는 별도 동의(기본 꺼짐)한 기여분만 보낸다. 딴짓 통계·앱 이름·창 제목·MBTI·출생연도는 방·프로필에 올리지 않는다. 캐릭터 그림은 자리표시이며 교체 절차는 `docs/art-assets.md`. 실제 RLS는 `supabase test db`로 사람이 확인(`docs/stage-4-verification.md` V4-1).
- 폰 앱(5단계): 안드로이드(APK) 먼저, 아이폰은 Unsupported. 구조는 옵션 A(Tauri 모바일 독립 프로젝트 `phone/` + Kotlin 차단 플러그인, 판정은 `crates/link-protocol`), 막히면 옵션 B(순수 Kotlin) 또는 폰 앱 분리(`docs/decision-phone-architecture.md` 되돌림 조건). 신호는 PC가 서버에 올리는 임대(lease)를 폰이 받는 방식이며 만료되면 풀린다. 푸시는 ntfy(FCM 아님), APK는 GitHub Releases 링크 QR(PC의 LAN 서빙 없음). 폰 점수 감점(O-23)은 실사용 후 결정.
- 공부 모드 기본 길이: 쉬움 25/5분(4세트마다 20분), 중간 50/15분, 어려움 90/10분 (PLAN.md 17번).
