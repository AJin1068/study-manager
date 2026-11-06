# 외부 반출 정책

앱 밖으로 나가는 것은 이 문서의 허용 목록뿐이다. 새 반출이 필요하면 이 문서와 `remote::payload`(또는 3단계의 `llm::egress`)의 타입을 같은 변경에서 추가하고 `egress_payload_guard`에 표본을 더한다. 근거: AGENTS.md §6, `docs/PLAN.md` 프라이버시 항목.

## 원칙

1. 탭 기록·창 제목·전체 URL·입력한 문장은 어떤 경로로도 나가지 않는다. 해당 필드가 타입에 존재하지 않는다.
2. 외부 연동은 꺼진 상태가 기본이다(`auto` = 설정이 완료되면 켜짐, 아니면 「설정 필요」). 모의 모드는 외부로 아무것도 보내지 않는다.
3. 외부 연동 실패는 잠금·타이머·차단을 약화시키지 않는다. 연동 실패 시 로컬 기능은 그대로다.
4. 사용자는 무엇이 나갔는지 볼 수 있다(설정 → 연동 → 내보낸 내용).

## 코드로 강제되는 것

| 장치 | 위치 | 효과 |
|---|---|---|
| 봉인된 `EgressPayload` | `remote::payload` | 요청 본문은 `Body::from_payload(&impl EgressPayload)`로만 만든다. 이 트레이트는 크레이트 밖에서 구현할 수 없고, 안에서도 검토된 구조체에 `egress_payload!`로만 붙인다. 임의의 `Serialize` 값은 컴파일되지 않는다(`compile_fail` 문서 시험 3개) |
| `Debug` 가림 | 같은 매크로 | 페이로드·`HttpRequest`·토큰의 `Debug`는 필드 값·경로·헤더를 찍지 않는다 |
| `HostPolicy` | `remote::http` | `UreqTransport`는 정책 없이 만들 수 없다. 제품: https와 설정된 엔드포인트 호스트 정확 일치(루프백 허용). 시험: `LoopbackOnly`. 위반은 소켓·DNS 이전에 `HostDenied`. `cfg(test)`가 아니므로 `tests/` 통합 시험에도 같은 규칙이 적용된다 |
| 리다이렉트 금지 | `UreqTransport` | 3xx를 따라가면 정책을 우회하므로 따르지 않는다 |
| 로그 | `remote::http` | 호스트·상태 코드·바이트 수·지연만. 경로(ntfy 토픽이 경로에 있다)·쿼리·헤더·본문 금지 |
| `remote_events` | DB(추가만 가능) | 시각·종류·결과·상태 코드·보낸 바이트 수만 기록. 내용 없음 |
| 비밀 | `remote::secrets`, `remote::secret_flow` | OS 키체인(불가하면 세션 한정 + 눈에 보이는 Degraded). 설정·`setting_history`·로그·DB에는 값도 해시도 없고 `secret_set`/`secret_cleared` 사실만 남는다 |
| 서버 쪽 | `supabase/` | 서버 표에 창 제목·URL·타이핑 컬럼이 없고, 정적 시험이 이름을 검사한다 |

## 시험

- `remote::payload::tests::egress_payload_guard_*`: 모든 페이로드 표본을 직렬화해 금지 키(`title`, `window_title`, `url`, `typed*`, `sentence*`)와 카나리 문자열(`CANARY_TITLE_7f3a`, `https://canary.example/secret-path`) 부재 단언. 표본 목록이 `egress_payload!` 사용 수와 같아야 통과한다.
- `llm::egress::tests::llm_payload_guard_*`: LLM 페이로드 표본에도 같은 금지 키·카나리 단언. `tests/llm_wire.rs`: 루프백 가짜 서버로 요청 모양·재시도·거부를 확인(실제 호스트·키 사용 없음).
- `tests/remote_wire.rs`: 루프백 가짜 서버가 받은 모든 본문에 카나리·금지 키 부재 단언, 리다이렉트 미추적, 외부 호스트 거부.
- `remote::secret_flow::tests`: 키 저장 뒤 settings·setting_history·remote_events·로그 어디에도 값 부재.
- `supabase/tests/schema.static.test.ts`: 저장소 전체에 JWT·service role 키·제공자 키 모양 문자열 부재.

## 허용 목록 (3~5단계 현재)

| 포트 | 보내는 것 | 타입 |
|---|---|---|
| Supabase 로그인 | 이메일, 6자리 코드, 리프레시 토큰 | `OtpRequest`, `OtpVerify`, `RefreshRequest` |
| Supabase 기기 | 기기 UUID, 앱 버전, 운영체제 이름 | `DeviceRegistration` |
| Supabase 신호 | 기기 UUID, 앱 버전, 공부 잠금 여부(불리언) | `PresenceBeat` |
| 계획 미러 | 계획 발생 목록(키, 시작 UTC 시각, 계획 이름, 과목, 세트 수, 상태: planned/skipped/canceled/moved)과 그 해시 | `SnapshotArgs`, `OccurrenceRow` |
| 계획 히스토리 | 계획 수정 기록(종류, 사유, 전후 값) 500건씩 | `PlanHistoryBatch`, `PlanHistoryRow` |
| 알림 채널 | 채널 종류, ntfy 토픽·서버, Telegram chat_id, 유예 분, UTC 오프셋 | `NotifyChannelRow` |
| 알림 시험 (ntfy) | 고정 시험 문장 한 줄. 받는 곳은 사용자가 설정한 ntfy 서버 | `NoticeText` |
| 알림 시험 (Telegram) | `{"action":"test"}` 하나. 봇 토큰은 서버에만 있고 Edge Function이 보낸다 | `NotifyTestRequest` |
| 알림 채널 `phone_app` (5단계) | `ntfy`와 같은 행·같은 토픽·서버(채널 종류만 다름). 알림 문구는 고정 시험 문장 또는 시각만 든 고정 문장이다 | `NotifyChannelRow`, `NoticeText` |
| LLM 위키 정리 | 과목 이름, 자료 조각(한 요청에 6,000자 이하, 사용자가 직접 고른 파일·붙여넣은 글), 고정 지시문 | `llm::egress::MessagesRequest` |
| LLM 위키 묻기 | 과목 이름, 질문(500자 이하), 로컬 검색으로 고른 위키 페이지 최대 3개(페이지당 8,000자 이하), 고정 지시문 | 같음 |
| LLM 목차 초안 | 과목 이름 | 같음 |
| LLM 연결 시험 | 고정 한 줄 | 같음 |
| LLM 공유 페이지 병합 (4단계, `SharedMerge`) | 페이지 이름(80자 이하), 방 구성원이 `allow_llm_merge`로 따로 동의한 기여 글(구성원당 6,000자 이하, 최대 6개; 글 이름 자리에 글쓴이 닉네임), 고정 지시문. 글쓴이 이름을 본문에 쓰지 말라는 지시를 포함 | 같음 |
| LLM 캐릭터 이름 (5단계, `CharacterNames`) | 후보 최대 4개의 카탈로그 라벨(종·털 색·표정·소품)과 고정 지시문. 사용자 이름·MBTI·출생연도·성별 필드는 없다 | 같음 |

LLM 요청은 `api.anthropic.com`으로만 가며 키는 헤더에만 있다(본문·로그·`remote_events`에 없음). 위키 폴더 전체, 페이지 목록, 창 제목, URL(링크 자료는 열지 않고 사용자가 쓴 메모만 보낸다), 입력한 문장은 보내지 않는다. 탭 판단은 `llm.judge_enabled`(기본 꺼짐) 동의가 있어야 하며 도메인·줄인 탭 제목(120자 이하)·공부 주제만 보낸다(경로·쿼리 없음, 저장 안 함). 병합·점검·문제 생성·자동 조정·챗봇·주간 인사이트도 같은 `MessagesRequest`로만 나간다(계획·집계·위키 본문, 창 제목·앱 이름 없음). 근거와 한도: `docs/decision-llm-egress.md`.

계획 이름·과목은 폰과 방에서 보기 위해 서버 미러에 올라가지만 **알림 문구에는 절대 넣지 않는다**(시각과 고정 문장만). 탭 기록·창 제목·URL·타이핑은 이 표 어디에도 없다.

서버 쪽(Edge Function) 규칙: ntfy 서버 주소는 https 공개 호스트 이름만(IP·localhost·내부 이름·포트·경로·인증 정보 거부). 이 검사는 `supabase/functions/_shared/channels.ts`에 있고 데스크톱과 같은 `fixtures/notify/contract.json`으로 시험한다.

## 스터디 방 (4단계)

Supabase RPC 인자 구조체이며 필드 이름이 RPC 매개변수 이름이다. 창 제목·URL·앱 이름·입력한 문장·MBTI·출생연도·성별 필드는 없다. 모두 `remote::payload`의 `egress_payload!` 타입이다.

| 포트 | 보내는 것 | 타입 |
|---|---|---|
| 방 만들기·참가·나가기 | 방 이름, 방 안에서 쓰는 닉네임, 초대 코드(자격증명: 호출 동안만 보유, `Debug` 가림), 방 ID | `CreateRoomArgs`, `JoinRoomArgs`, `RoomIdArgs` |
| 초대 | 방 ID, 유효 시간, 최대 사용 횟수 / 취소할 초대 ID | `CreateInviteArgs`, `RevokeInviteArgs` |
| 내보내기 | 방 ID, 대상 사용자 ID | `KickMemberArgs` |
| 방 상태 | 잠금 단계 문자열, 남은 초, 레벨, 오늘 방해 횟수(사용자가 공유를 켰을 때만) | `StatusReportArgs` |
| 하루 요약 | 로컬 날짜(YYYY-MM-DD), 공부 분, 계획 분 | `DailyReportArgs` |
| 공개 설정 | 레벨·상태·주간 시간·시도 횟수 공유 여부, 방해 알림 발행·수신 여부(불리언 6개) | `FriendPrefsArgs` |
| 시간표 공유 | 방 ID, 방장이 공유한 블록 목록(시작 UTC 시각, 계획 이름, 과목, 세트 수) | `ShareScheduleArgs`, `ScheduleItemRow` |
| 방해 알림 | 마지막으로 받은 시각(조회) / 인자 없음(발행) | `PollDistractionArgs`, `NoArgs` |
| 착용 아이템 공개 | 캐릭터 종과 슬롯별(모자·상의·장신구·손) 착용 아이템 ID. 이름·보유만 한 아이템·가격 없음 | `InventoryPublicArgs` |
| 목록·스냅샷 조회 | 인자 없음 또는 방 ID | `NoArgs`, `RoomIdArgs` |

## 프로필·잔디·방명록·선물 (4단계)

`profile::payload`의 `egress_payload!` 타입이다. 범위가 비공개인 구역은 `publish::settings_args`가 비워서 보낸다. 시도 횟수·앱·창 제목·URL·입력한 문장·위키 본문·포인트 필드는 없다.

| 포트 | 보내는 것 | 타입 |
|---|---|---|
| 프로필 설정 | 표시 이름(사용자가 입력), 구역별 공개 범위 9개, 레벨, 업적 배지 ID, 보유 가구 ID, 사용자가 공개로 고른 위키 페이지 **이름**, 현재 과목 | `SetProfileArgs` |
| 연간 잔디 | 날짜별 공부 분 | `GrassArgs`, `GrassEntry` |
| 프로필 조회 | 대상 사용자 ID | `MemberArgs` |
| 방명록 | 주인 ID, 한 줄 글 / 삭제·신고할 글 ID | `SignGuestbookArgs`, `EntryIdArgs` |
| 선물 | 멱등 키, 받는 사람 ID, 아이템 ID, 짧은 메시지 / 키·선물 ID로 상태·수락·거절 | `SendGiftArgs`, `GiftKeyArgs`, `GiftIdArgs` |

## 공유 위키 (4단계)

`shared_wiki::payload` 타입이다. 사용자가 공유로 표시한 페이지만 `shared_wiki::service`가 만들 수 있다. 위키 폴더 전체·공유하지 않은 페이지·창 제목·URL은 보내지 않는다.

| 포트 | 보내는 것 | 타입 |
|---|---|---|
| 공유 페이지 게시 | 방 ID, 페이지 이름, **페이지 본문**, 기준 리비전 ID, LLM 병합 동의 불리언 | `PublishSharedPageArgs` |
| 공유 철회 | 방 ID, 페이지 이름 | `WithdrawSharedPageArgs` |
| 병합본 저장 | 방 ID, 페이지 이름, 병합된 **본문**, 합친 리비전 ID 목록 | `SaveMergedPageArgs` |
| 공유 페이지 목록 | 방 ID | `RoomIdArgs` |

공유 페이지 병합용 LLM 요청(`SharedMerge`)은 위 LLM 표를 따른다.

## 폰 (5단계)

| 포트 | 보내는 것 | 타입 |
|---|---|---|
| 폰 짝짓기 | 1회용 코드(5분) | `phone::payload::ClaimPairArgs` (폰이 보냄) |
| 폰 잠금 신호 | 실행·구간 ID, seq, 상태(locked/free), 해제 사유, 만료 시각 | `PublishLockArgs` (PC가 보냄) |
| 폰 상태 | 받은 seq, 잠금 불리언, 차단 서비스 켜짐 불리언 | `BeatArgs` (폰이 보냄) |
| 집어 든 횟수 | 구간 ID, 횟수 | `PickupArgs` (폰이 보냄) |
| 폰 알림 수신 (`phone_app`) | 폰이 사용자 ntfy 서버에 GET으로 묻는다: 토픽(경로에 있음)과 마지막 메시지 ID 또는 시각. 본문 없음. 응답은 고정 문구만 화면에 보인다 | `phone::push::NtfyPush` (HTTP 포트, `HostPolicy` 적용) |
| 짝 코드 발급·해제·상태·잠금 조회 | 인자 없음(서버는 로그인한 계정으로 짝을 안다) | `NoArgs` |

앱 이름·패키지명·사용 내역·창 제목·URL 필드는 어떤 타입에도 없다(`link-protocol`의 보고 타입은 알 수 없는 필드를 거부). 허용 앱 목록은 폰 로컬에만 있고 서버에 가지 않는다. 시험: `phone::publisher_tests`(요청 본문 카나리 grep), `phone::publisher_tests::e2e`, `fixtures/phone/privacy.static.test.ts`.

## 잠금 중 설정 변경

계획된 공부 진행 중에는 모든 설정이 잠긴다(D-1). 예외는 반출을 줄이는 방향뿐이며 `setting_history`에 `applied_privacy_tighten`으로 남는다: 모의 모드로 전환, 알림 채널 끄기, 비밀 삭제, 로그아웃. 키 입력·로그인·모드를 실제로 되돌리기는 잠긴다(시도는 `rejected_locked`로 기록).
