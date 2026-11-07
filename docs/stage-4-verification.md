# 4단계 검증 기록

4단계(스터디방) 전체(S4.1~S4.12)의 검증 문서다. 에이전트는 실제 Supabase 프로젝트·계정 두 개 이상, Postgres(Docker/Supabase CLI), 폰 카메라, 웹 호스팅, GUI, Windows 환경이 없으므로 아래 「검증 안 됨」 항목은 실행하지 않았다. 4단계 완료 기준(친구 2명 이상이 같은 방에서 공부함)은 V4-1, V4-2를 사람이 확인하기 전까지 충족된 것으로 표기하지 않는다. 세부 설계 메모는 `docs/stage-4-rooms.md`, `docs/stage-4-shared-wiki.md`, 그림 교체 절차는 `docs/art-assets.md`에 있다.

## 환경

- macOS(Darwin 23.4.0, arm64), 로컬 개발 머신. 실제 Supabase·Anthropic 키 없음, 외부 호스트 호출 없음(시험은 `FakeBackend`의 인메모리 서버, `MockLlm`, 스크립트 전송, 루프백 가짜 서버, 가짜 fetch만 사용).
- Docker·Supabase CLI 없음: 서버 SQL(`supabase/migrations/*`)과 pgTAP(`supabase/tests/rls_s4*.sql`)은 한 번도 실행되지 않았다. 정적 문자열 시험(`supabase/tests/*.static.test.ts`)만 돌아간다.
- GUI 실행 불가, 폰 카메라(QR) 없음, 웹 호스팅 계정 없음, Windows 실기기·Windows 타깃 빌드 환경 없음(2단계에서 번들 SQLite C 헤더 부재로 `cargo check --target x86_64-pc-windows-msvc`가 실패했다).
- 캐릭터·옷 그림은 오리지널 **자리표시 SVG 도형**이다(O-13). 실제 그림 자산은 없다.
- 새 로컬 마이그레이션: m020 `rooms_local`(room_membership_cache, room_prefs, together_imports), m021 `character`(character_profile, wardrobe), m022 `shop`(inventory_local), m023 `shared_wiki_local`(shared_page_marks, shared_concept_seen), m024 `profile_gift`(profile_local, profile_visibility, profile_wiki_public, gift_outbox, gift_received).
- 새 서버 마이그레이션(Supabase에 적용된 적 없음): `20260601000200_s4_rooms.sql`, `20260601000300_shared_wiki.sql`, `20260601000400_s4p_profile.sql`.
- 새 웹 번들: `web/`(+ `vite.web.config.ts`, `pnpm build:web`). 확장 번들은 변경 없음.

## 스토리 상태

코드 상태는 소스에서 확인한 내용이다(모듈·시험 존재 확인). 시험 통과 여부는 아래 「게이트 명령과 결과」가 리더 실행 결과로 채워지기 전까지 확인된 것이 아니다.

| 스토리 | 내용 | 코드 상태 | 자동 증명(위치) |
|---|---|---|---|
| S4.1 | 서버 방 스키마·RLS·FakeBackend 방 의미 | 구현. **SQL은 실행된 적 없음** | `fixtures/rooms/rls-matrix.json`을 Rust(`remote/fake/rooms.rs`, `rooms/scenarios.rs`)와 TS가 같이 단언, `supabase/tests/rooms.static.test.ts`(RLS 활성·정책 부재·`security definer`+`search_path` 고정·`REVOKE`), `supabase/tests/rls_s4.sql`(pgTAP, 미실행) |
| S4.2 | 캐릭터 코어·첫 실행 설정 | 구현(그림은 자리표시). 이름·소개는 내장 사전 또는 LLM(`CharacterNames`) | `character::tests`(같은 입력+시드→같은 후보, 모두 건너뛰기, 다시 뽑기 비파괴, 레벨→성장 단계 경계 0/49/50/199/200/499/500, 금칙어, 모든 종×옷 렌더 치환 잔여 0), `character-contract.json` |
| S4.3 | 말투 기본값 연결 | 구현 | `character::tests`: MBTI→tone 표 완전·독설 없음, 첫 선택에서 한 번만 적용, 사용자 값·잠금 시 덮어쓰지 않음 |
| S4.4 | 방 생성·초대·QR·입장·나가기 | 구현. **방장을 지정해 넘기는 명령은 없음**(방장이 나가면 가장 오래된 멤버에게 넘어가고 마지막 사람이 나가면 방이 닫힘) | `rooms::code`(10자 base32, `studymanager://join?c&e&k` 왕복, 형식 위반·비 https·짧은 코드 거절), `rooms::scenarios`(만료·폐기·사용 횟수·이미 멤버·정원 8·강퇴 후 재입장 차단), `rooms-contract.json` |
| S4.5 | 실시간 상태·랭킹·같이 공부 | 구현(폴링 기본: 방 화면 5초/닫힘 30초) | `rooms::view`(90초 후 offline), `rooms::ranking` 골든, `rooms::feed`, `rooms::status`(본인 값만 송신), `rooms::together`(미리보기 후 일반 plan API로 가져오기, 자동 적용 없음) |
| S4.6 | 친구 공개 설정·딴짓 알림(기본 꺼짐) | 구현 | `rooms::prefs`: 기본 `share_level`/`share_status`만 켜짐, 나머지 꺼짐, 켜기는 run 중 거절·끄기는 `applied_privacy_tighten`으로 허용, 딴짓 알림은 별명·시각만 |
| S4.7 | 공유 위키 | 구현 | `shared_wiki::scenarios`·`merge`·`payload`·`concepts`·`quiz`·`summary`(공유 표시 없는 페이지는 어떤 payload에도 없음, 동의 없는 기여분은 모델 입력에서 제외, 인젝션 페이지, 철회, 동시 편집은 분기, 크기·개수 상한, 비멤버 0), `supabase/tests/shared_wiki.static.test.ts`, `rls_s4_shared_wiki.sql`(미실행) |
| S4.8 | 캐릭터 상점·옷 입히기 | 구현(그림은 자리표시) | `shop::tests`(잔액 부족·중복·동시 구매 멱등, 업적 조건 골든, 업적 전용 아이템 표시, 원장 합계 불변식), `shop-contract.json` |
| S4.9 | 프로필 홈·방명록·업적 배지 | 구현 | `profile::tests`(칸×범위 매트릭스, 비공개 칸은 저장·송신 0 카나리, 방명록 제한, 딴짓 통계 필드 없음), `fixtures/profile/rls-matrix.json`, `supabase/tests/profile.static.test.ts`, `rls_s4_profile.sql`(미실행) |
| S4.10 | 옷·가구 선물 | 구현. 환불은 확정 거절, 또는 `gift_status`가 성공적으로 「없음」을 돌려주고 마지막 시도 후 `REFUND_GRACE_MS`(10분) 이상 지난 경우에만 1회 | `profile::tests`(모호 실패 후 서버 성공 시 환불 0·선물 1건, 확정 거절 후 환불 정확히 1회, 이중 환불 불가, 일일 한도 5, 메시지 60자, 비멤버 거절) |
| S4.11 | 웹 프로필 링크·웹 대시보드 | 구현(호스트 무관 정적 번들, 모의 어댑터 포함) | `web/src/web.test.tsx`(설정 실패 시 닫힘, 16 hex 링크만, 익명 호출에 사용자 토큰 미전송, 알 수 없는 링크·빈 프로필, 알 수 없는 칸 폐기·방명록 미표시, 로그인 필요, 만료 토큰 로그아웃, 앱·제목·딴짓 필드 렌더 없음), `web/tests/bundle.static.test.ts`(`service_role`·비밀 부재), `pnpm build:web` |
| S4.12 | (선택) Realtime 웹소켓 어댑터 | **부분**: Phoenix 채널 코덱·세션 상태기계(하트비트 30초, 침묵 65초 뒤 끊김)·인메모리 전송 쌍만 있고 **웹소켓 크레이트와 소켓 전송이 없다**. 폴링이 유일한 실제 피드 | `rooms::realtime` 시험(변경 알림이 `PollingFeed::poll_now`만 깨움). 4단계 완료 기준에 영향 없음 |

## 게이트 명령과 결과

아래 칸은 리더가 실제 실행 결과로 채운다. 에이전트는 이 문서를 쓸 때 빌드·시험을 돌리지 않았다.

| 명령 | 위치 | 결과 |
|---|---|---|
| `pnpm tsc --noEmit` | 루트 | 종료 0 |
| `pnpm lint` | 루트 | 종료 0 |
| `pnpm test` (vitest, `supabase/**`·`web/**` 포함) | 루트 | 395개 통과 |
| `cargo clippy --all-targets -- -D warnings` | `src-tauri` | 종료 0 |
| `cargo test` | `src-tauri` | 951개 통과 (+통합·doc 별도) |
| `pnpm build:web` | 루트 | 종료 0 |
| Windows 네이티브(x86_64-pc-windows-msvc, Windows 11 x64, rustc 1.99.1) `cargo test` | `src-tauri` | 종료 0: lib 1047 + llm_wire 4 + remote_wire 7 + doc 3 통과, 실패 0 (2026-11-07). 이전 기록: 「Windows 타깃 `cargo check` 종료 101: 번들 SQLite C 헤더 부재 (검증 안 됨)」 — 네이티브 Windows 빌드에서는 재현되지 않았다. `cfg(unix)` 시험(심볼릭 링크·유닉스 소켓)은 컴파일되지 않아 실행되지 않았다 |
| Windows 네이티브 `cargo clippy --all-targets -- -D warnings` | `src-tauri` | 종료 0 (2026-11-07, 수정 커밋 a22bb03: `browser_ipc.rs`의 `SOCKET_FILE`을 `cfg(unix)`로) |
| `cargo fmt --check` | `src-tauri` | 종료 0 (2026-11-07, Windows, 커밋 cd88e14로 `cargo fmt` 적용 후). 이전 기록: 「실행 불가: rustfmt 미설치 (검증 안 됨)」 |

`supabase test db`는 Docker·Supabase CLI가 없어 게이트에 넣지 못했다(V4-1).

## 에이전트가 자동으로 증명한 것

- **방 권한 모델(FakeBackend 한정).** 같은 RLS 매트릭스 fixture(`fixtures/rooms/rls-matrix.json`, `profile/`, `shared_wiki/`)를 인메모리 서버 시험이 단언한다: 방 관련 표는 클라이언트 직접 읽기·쓰기가 없고 모든 쓰기는 `{"code": ...}`를 돌려주는 RPC, 읽기는 `room_snapshot`/`list_my_rooms`뿐이며 멤버의 공개 설정에 따라 숨긴 칸이 `null`로 도착한다. 이것은 **Postgres RLS의 증명이 아니다**(V4-1).
- **초대·입장.** 만료 24시간·최대 10회·방장 폐기, 사용자당 분당 5회 입장 시도(실패 기록 유지), 정원 8, 강퇴 = 차단, 이미 멤버, 코드 엔트로피·형식, 코드는 해시만 서버 보관·표시 1회. 초대 링크 왕복과 변조 거절(비 https 주소, 키 누락, 짧은 코드, 다른 경로).
- **상태·랭킹.** 90초 침묵 뒤 offline, 폴링 실패 시 마지막 값 유지, 랭킹 산수 골든(자정 경계), 본인 값만 송신, 분 단위 상한(0..1440)과 증가량 급증 거절(O-21).
- **같이 공부.** 방장이 공유한 시간표(최대 20블록, 7일)를 멤버가 미리보기 후 일반 plan API로 가져오며 잠금·겹침 검사·히스토리 행이 그대로 적용된다.
- **공개 설정.** 기본값 보수적, 끄면 서버에서 해당 칸이 삭제·`null`, run 중 켜기 거절. 딴짓 알림은 보내는 쪽 공개와 받는 쪽 수신이 둘 다 켜진 경우만, 켜기 이전 알림은 재생하지 않음. 앱 이름·창 제목·딴짓 통계는 어떤 방 payload에도 필드가 없다.
- **공유 위키.** 공유는 확인 화면을 거쳐야 하고 합치기 동의(`allow_llm_merge`)는 별도·기본 꺼짐. 합치기 입력 타입(`ConsentedPart`)은 로컬 비공유 텍스트로 만들 수 없다. 동의 없는 기여분은 모델 호출 본문에 카나리 문자열이 나타나지 않음. 합치기 결과는 기여자 목록을 유지하고 원본 revision을 지우지 않는다. 철회하면 서버 본문이 사라지고 새 revision이 생기지 않으며 친구 로컬 사본이 정리된다.
- **캐릭터·말투.** 후보 결정성, 건너뛰기, 비파괴 다시 뽑기, 성장 단계 경계, 금칙어 걸러내기(라벨·내장 이름·모델이 지은 이름). MBTI·출생연도·성별은 서버 payload에 없다(`remote::payload` 금지 키 가드). MBTI→말투 초기값은 한 번만, 사용자 값 우선.
- **상점·선물.** 구매는 `point_ledger` 음수 행으로 멱등(`buy:<item>:<n>`), 잔액=Σdelta 불변식, 포인트는 전송되지 않음, 업적 전용 아이템 선물 불가. 선물은 로컬 차감과 `gift_outbox` 기록이 한 트랜잭션이고, 모호한 실패는 환불하지 않고 pending, 확정 거절은 환불 1회, `resolve_outbox`가 pending 행만 바꿔 이중 환불을 막는다.
- **프로필·웹.** 칸×범위×조회자 매트릭스, 비공개 칸의 저장·송신 0, 익명 RPC는 `public` 칸만, 웹은 `service_role`·비밀을 번들에 담지 않고 익명 호출에는 사용자 토큰을 붙이지 않는다.
- **Rust↔TS 형태 고정.** `rooms-contract.json`, `character-contract.json`, `shop-contract.json`, `shared-wiki-contract.json`, `profile-contract.json` 공유 fixture와 `src/api/*.test.ts`, `src/pages/*View.test.ts`.

## 검증 안 됨

| ID | 스토리 | 항목 | 안 된 이유 | 사람이 하는 한 줄 절차 |
|---|---|---|---|---|
| V4-1 | S4.1, S4.7, S4.9 | 실제 Postgres에서 서버 SQL이 적용되는지와 RLS·RPC 권한(pgTAP `rls_s4.sql`, `rls_s4_shared_wiki.sql`, `rls_s4_profile.sql`): 비멤버·다른 방 멤버·익명이 아무 행도 못 읽는지, `REVOKE EXECUTE`, 입장 시도 카운터가 롤백되지 않는지, 익명 `get_public_profile`이 `public` 칸만 돌려주는지, 성능 | [판정 2026-11-07: 부분] Docker·Supabase CLI·Postgres 없음. 지금까지 SQL은 문자열 정적 시험만 통과 | Docker가 있는 PC에서 `supabase start` 후 `supabase test db`를 돌려 세 pgTAP 파일이 모두 통과하는지 보고, 실패한 단언이 있으면 고친다 |
| V4-2 | S4.4, S4.5, S4.6, S4.10 | 실제 Supabase 프로젝트에서 두 계정(A, B)이 방을 만들고 초대 코드로 입장해 상태·랭킹이 보이는 흐름, 초대 링크의 endpoint·anon key 자동 채움, 폴링 지연, 딴짓 알림 opt-in, 선물 전송·수락 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실제 프로젝트·계정 둘·기기 둘 없음 | 세 마이그레이션을 적용한 프로젝트에 계정 둘로 로그인해 A가 방 생성→초대, B가 입장→공부 시작 후 A 화면에 「공부 중」이 5~10초 안에 뜨고, B에게 선물을 보내 수락까지 확인한다 |
| V4-3 | S4.2, S4.8, S4.9 | 실제 그림 자산(현재는 자리표시 도형)의 시각 품질, 옷이 모든 종에 맞게 정렬되는지, 캐릭터 선택·상점·프로필 홈·방명록·공부 잔디·방 화면의 실제 렌더와 사용성 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 그림을 그릴 수 없고 GUI 실행 불가 | `docs/art-assets.md` 절차로 `parts.json`·`items.json`의 `svg`를 교체한 뒤 첫 실행 선택, 상점 착용, 프로필 홈을 라이트·다크에서 눈으로 확인 |
| V4-4 | S4.7 | 실제 모델의 공유 페이지 합본 품질, 인젝션 페이지에 대한 실제 모델 반응, 서버에서의 실제 revision 동시성 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실제 모델·서버 없음(합치기는 `MockLlm`, 서버는 인메모리) | 두 계정이 같은 개념의 페이지를 각자 공유(합치기 동의 포함)하고 한 쪽이 합치기를 실행해 기여자 표시와 내용이 납득되는지, 동시에 올렸을 때 revision이 분기로 남는지 확인 |
| V4-5 | S4.11, S4.12 | (a) 웹 번들의 실제 호스팅 배포와 프로필 링크가 실제 도메인에서 열리는지, `config.json` 주입, OTP 로그인 메일, (b) Supabase Realtime에 실제 웹소켓으로 붙기(전송 계층 자체가 아직 없음) | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 호스팅·도메인·메일 계정 없음. 웹소켓 크레이트와 소켓 전송 미구현 | (a) `pnpm build:web` 산출물을 정적 호스팅에 올리고 `config.json`을 채워 프로필 링크를 익명 창에서 열고 본인 로그인으로 대시보드를 본다. (b) 소켓 전송을 구현한 뒤에만 가능하며, 지금은 폴링이 기본이므로 건너뛴다 |
| V4-6 | S4.4 | 폰 카메라로 QR을 읽어 `studymanager://join?...` 딥링크가 앱을 열고 입장 화면이 채워지는지(딥링크 OS 등록 포함) | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 폰·카메라·OS 딥링크 등록 확인 불가 | 방의 초대 QR을 다른 기기 카메라로 스캔하거나 링크를 붙여넣어 앱이 열리고 코드·주소가 채워지는지, 확인 후 입장되는지 본다 |
| V4-7 | S4.2 | 실제 모델이 짓는 캐릭터 이름·한 줄 소개의 품질과 금칙어 걸러내기 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실제 키 없음(내장 사전 경로만 시험됨) | LLM 키를 넣고 캐릭터를 몇 번 다시 뽑아 이름이 어색하거나 기존 캐릭터와 겹치지 않는지 확인 |
| V4-8 | S4.4, S4.5, S4.8, S4.9, S4.10 | 방·캐릭터·상점·프로필·선물·공유 위키 UI의 실제 동작, 트레이 캐릭터 한 줄 표시, 선물 상자 알림 | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] GUI 실행 불가(타입 체크·린트·순수 뷰 로직 시험만) | 앱을 띄워 방 만들기→초대→랭킹, 상점 구매→선물, 공유 위키 올리기→철회를 화면에서 한 번씩 해 보고 오류 문구가 이해되는지 확인 |
| V4-9 | S4.1~S4.10 | 서버 마이그레이션 세 개가 빈 프로젝트에 순서대로 오류 없이 적용되는지, 기존 2단계 스키마와 충돌이 없는지 | [판정 2026-11-07: 부분] 실제 Postgres 없음 | 빈 Supabase 프로젝트에 `supabase db push`로 5개 마이그레이션을 순서대로 적용해 오류가 없는지 확인 |
| V4-10 | 전체 | Windows: 4단계 신규 Rust 코드의 Windows 타깃 `cargo check/test`, 딥링크 등록, 웹뷰 렌더 | [판정 2026-11-07: 부분] Windows 기기·타깃 빌드 환경 없음(macOS에서만 시험) | Windows PC에서 `cargo test`와 `cargo clippy --all-targets -- -D warnings`를 돌리고 방 화면과 캐릭터 선택이 뜨는지 확인 |
| V4-11 | S4.5 | 실제 네트워크 지연·시계 오차에서의 상태 신선도(90초 offline 판단), 랭킹의 날짜 경계(타임존) | [판정 2026-11-07: 사람 확인(MANUAL-CHECKLIST)] 실제 서버·시계 오차 환경 없음(가짜 시계만) | 서로 다른 시간대 설정의 두 PC로 자정 전후에 공부해 오늘 랭킹이 각자 자기 날짜로 집계되는지 확인 |

### Windows PC 검증 결과 (2026-11-07)

환경: Windows 11 x64, rustc 1.99.1. 스텁 PostgreSQL 16.4(실제 Supabase 아님: GoTrue·PostgREST·Realtime·Vault·`pg_cron` 없음, pgTAP 대신 `pgtap-lite`). 위 체크리스트는 바꾸지 않았다.

| ID | 판정 | 명령 | 결과(종료코드·개수) | 비고 |
|---|---|---|---|---|
| V4-1 | 부분 | `supabase/tests/stub/run.ps1` (스텁 PG 16.4에 마이그레이션 8개 적용 후 `rls_s*.sql` 실행) | 종료 0: 마이그레이션 오류 없음, rls_s4 26/26, rls_s4_profile 27/27, rls_s4_shared_wiki 22/22 통과(전체 7파일 172단언). 음성 대조군은 요구대로 실패(1건) | 스텁이라 부분. 실제 `supabase start`/`supabase test db`·성능은 미실행(Docker/WSL2 없음) |
| V4-2 | 사람 확인(MANUAL-CHECKLIST) | 실행 안 함 | — | 실제 프로젝트·계정 둘·기기 둘 필요 |
| V4-3 | 사람 확인(MANUAL-CHECKLIST) | 실행 안 함 | — | 그림 자산 없음, GUI 시각 판단 |
| V4-4 | 사람 확인(MANUAL-CHECKLIST) | 실행 안 함 | — | 실제 모델·서버 필요 |
| V4-5 | 사람 확인(MANUAL-CHECKLIST) | `pnpm build:web` | 종료 0 (산출물 생성만) | (a) 호스팅·도메인·메일은 사람. (b) Realtime 소켓 전송은 미구현이라 건너뜀 |
| V4-6 | 사람 확인(MANUAL-CHECKLIST) | 실행 안 함 | — | 폰 카메라·OS 딥링크 등록 필요 |
| V4-7 | 사람 확인(MANUAL-CHECKLIST) | 실행 안 함 | — | 실제 LLM 키·모델 품질 판단 |
| V4-8 | 사람 확인(MANUAL-CHECKLIST) | `pnpm tsc --noEmit`, `pnpm lint`, `pnpm test` | 종료 0, 0, 72개 파일 511개 통과 | 정적·순수 로직 시험만. GUI 클릭 확인은 하지 않음 |
| V4-9 | 부분 | `supabase/tests/stub/run.ps1` | 종료 0: 서버 마이그레이션 8개가 빈 스텁 PG 16.4에 오류 없이 순서대로 적용됨 | 스텁 PG라 부분. 빈 Supabase 프로젝트에 `supabase db push`는 미실행 |
| V4-10 | 부분 | `cargo test` / `cargo clippy --all-targets -- -D warnings` / `cargo fmt --check` (`src-tauri`, 네이티브 x86_64-pc-windows-msvc) | 종료 0 / 0 / 0: lib 1047 + llm_wire 4 + remote_wire 7 + doc 3 통과, 실패 0 | 수정 커밋 a22bb03(`SOCKET_FILE` `cfg(unix)`), cd88e14(`cargo fmt`·`llm::config` 시험의 OS별 절대경로). `cfg(unix)` 시험은 컴파일되지 않음. 딥링크 등록·방 화면·캐릭터 선택 렌더는 미확인(사람) |
| V4-11 | 사람 확인(MANUAL-CHECKLIST) | 실행 안 함 | — | 서로 다른 시간대 두 PC·실제 서버 필요 |

## 단계 완료 기준 체크리스트 (PLAN 문구 그대로)

- [ ] 친구 2명 이상이 같은 방에서 공부함 — `FakeBackend` 다중 사용자 시나리오와 권한 로직, 어댑터 와이어는 자동 증명. 실제 RLS(V4-1)와 실제 2명 이상 사용(V4-2)을 사람이 확인하기 전까지 미충족.

## 알려진 잔여 위험 (수용·추적)

- **실제 RLS는 한 번도 실행되지 않았다.** 인메모리 서버는 SQL이 의도한 규칙을 되풀이할 뿐 SQL이 그렇게 동작함을 보이지 않는다. 정적 시험은 `enable row level security`, 정책 부재, `security definer`+`search_path` 고정, `REVOKE`의 문자열 존재만 확인한다. 이 문서의 가장 큰 위험이며 V4-1이 첫 번째 사람 확인이다. 조기 신호: 비멤버 `select`가 비어 있지 않음.
- **랭킹 값은 각 PC가 보고한 값이다(O-21).** 분 단위 상한과 증가량 급증 거절 외에 부정 방지는 하지 않는다. 친구 방 전제다.
- **초대 코드는 약 50bit다.** 해시 저장·분당 5회 시도 제한·24시간 만료·10회 사용·방장 폐기·강퇴 차단이 방어선이며, 유출된 유효 코드는 만료·폐기 전까지 입장에 쓸 수 있다. 코드는 UI에만 표시되고 로그에 남기지 않는다.
- **초대 페이로드의 anon key.** 링크·QR에 endpoint와 anon key가 들어간다(비밀 아님, 프로젝트 소유자가 비용·무료 한도를 책임). 받는 쪽은 사용자 확인 뒤에만 채워지고 https가 아닌 주소는 거절된다.
- **철회는 이미 본 사람 사본을 회수하지 못한다.** 공유 위키 철회 후에도 친구가 이미 본 내용은 남는다(UI 고지). 합치기는 실행자의 PC·키로 다른 멤버의 동의된 텍스트를 Anthropic에 보내므로 동의 구조(별도 플래그, 기본 꺼짐)에만 의존한다. 합본 품질은 모델에 달렸다(V4-4).
- **선물 환불은 보수적이다.** 모호한 실패는 `gift_status`가 「없음」을 돌려주고 10분 이상 지난 뒤에야 환불한다. 그 사이 포인트는 차감된 채 pending이다. 서버 상태 조회가 계속 실패하면 pending이 오래 남을 수 있다.
- **방장을 직접 지정해 넘길 수 없다.** 방장은 나가거나 계정을 지울 때만 서버 정책(가장 오래된 멤버 승격, 빈 방 삭제)으로 바뀐다. 계획의 「방장 이양」 중 지정 이양 명령은 구현되지 않았다.
- **자리표시 그림.** 캐릭터·옷은 색 도형이다. 교체는 데이터 파일 두 개이지만 통짜 이미지(PNG)로 바꾸려면 `render.rs` 합성 변경이 필요하다(`docs/art-assets.md`). 오리지널 원칙의 금칙어 목록은 완전하지 않아 외주 시안은 사람이 검토해야 한다.
- **Realtime 소켓이 없다.** 폴링 5초(방 화면)/30초가 유일한 피드다. 코덱·상태기계는 시험되었지만 연결되지 않은 코드이므로 V4-5(b)는 구현 이후의 일이다.
- **웹 대시보드는 서버에 이미 있는 집계만 본다.** 앱 이름·창 제목·딴짓 통계 필드가 없다. 로그인 토큰은 브라우저 세션 저장소에 둔다(탭을 닫으면 사라짐).
- **Windows 미검증이 누적된다**(V4-10, 3단계 V3-12 포함).
- **3단계에서 이어지는 항목**: 조기 해제 시도 소진 정책(O-26), PDF 추출기 미연결(V3-3), 창 제목 LLM 분류 호출부 없음. 4단계에서 바뀌지 않았다.

## 개인정보 검증 요약

- 방 payload는 봉인된 `EgressPayload` 구조체로만 만든다(`remote::payload`의 방 인자들, `profile::payload`, `shared_wiki::payload`). 금칙 키(`title`, `window_title`, `url`, `typed*`, `sentence*`)와 카나리(`CANARY_TITLE`, `CANARY_URL`) 표본 가드를 통과한다. 로컬 마이그레이션 m020~m024도 `privacy_schema_guard` 대상이다.
- 방·프로필 payload에 앱 이름·창 제목·딴짓 상세·MBTI·출생연도·성별 필드가 없다. 딴짓 통계는 방·프로필 어디에도 올라가지 않는다(알림은 별명·시각만, 보내는 쪽 공개와 받는 쪽 수신이 모두 켜진 경우).
- 공유 표시가 없는 위키 페이지는 어떤 경로로도 payload와 모델 입력에 없다(카나리 시험). 프로필의 비공개 칸은 저장도 송신도 되지 않는다.
- 새 LLM 호출: 공유 위키 합치기(`SharedMerge`, 동의된 공유 텍스트만), 캐릭터 이름·소개(`CharacterNames`, 외형 부품 이름만). 둘 다 `api.anthropic.com`뿐이고 키는 헤더에만 있다. **`docs/egress-policy.md`에는 이 문서 작성 시점에 4단계 반출 행(방·프로필·공유 위키·상점·선물 RPC, `SharedMerge`, `CharacterNames`)이 아직 추가되지 않았다.** 4단계 커밋 전에 같은 변경에서 추가해야 한다(계획 §4단계 산출).
- 새로 Supabase로 나가는 것: 방 상태(공부 중/휴식/남은 초/레벨/오늘 시도 횟수는 공개 설정이 켜진 경우만), 일일 합계(분), 친구 공개 설정, 공유한 시간표(이름·과목·세트), 인벤토리·프로필 공개 칸, 방명록, 선물, 공유 위키 revision 본문(공유 표시된 것만). 모두 사용자가 켠 칸만이며 기본은 레벨·상태뿐이다.
- 기본값은 보수적이다: `friends.share_level`·`friends.share_status`만 켜짐, `share_week_time`·`share_attempts`·`distraction_publish`·`distraction_receive` 꺼짐, 프로필 칸 범위 전부 비공개, 공유 위키 합치기 동의 꺼짐. 켜는 방향은 공부 run 중 잠기고 끄는 방향은 허용되어 `applied_privacy_tighten`으로 기록된다.
- 웹 번들에는 `service_role`·비밀이 없고, 익명 프로필 호출에 사용자 토큰을 붙이지 않는다.

## 5단계로 넘기는 항목

- 폰 앱의 짝짓기·임대 모델은 2단계 서버 스키마와 4단계 방 스키마 위에 새 마이그레이션(서버는 `20260601000400` 뒤 타임스탬프, 로컬은 m024 뒤)으로 붙인다.
- 방·프로필 RPC 계열의 `REVOKE`/`security definer` 패턴과 반환 코드 규약(`{"code": ...}`)을 폰 RPC에서도 따른다. V4-1에서 pgTAP이 실패하면 5단계 시작 전에 같은 패턴을 먼저 고친다.
- 초대 딥링크(`studymanager://join`)의 OS 등록은 아직 확인되지 않았다(V4-6). 폰 앱은 별도 스킴을 쓴다.
- 웹 번들(`web/`)과 `vite.web.config.ts`는 그대로 두고, 폰 APK 다운로드 안내 페이지가 필요하면 같은 번들 설정 방식(`config.json`)을 재사용한다.
- 열린 항목: Realtime 소켓 전송(V4-5), 방장 지정 이양 명령, `docs/egress-policy.md` 4단계 행 추가, 실제 그림 자산(V4-3).
