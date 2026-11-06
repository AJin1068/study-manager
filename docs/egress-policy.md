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

## 허용 목록 (3단계 현재)

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

| LLM 위키 정리 | 과목 이름, 자료 조각(한 요청에 6,000자 이하, 사용자가 직접 고른 파일·붙여넣은 글), 고정 지시문 | `llm::egress::MessagesRequest` |
| LLM 위키 묻기 | 과목 이름, 질문(500자 이하), 로컬 검색으로 고른 위키 페이지 최대 3개(페이지당 8,000자 이하), 고정 지시문 | 같음 |
| LLM 목차 초안 | 과목 이름 | 같음 |
| LLM 연결 시험 | 고정 한 줄 | 같음 |

LLM 요청은 `api.anthropic.com`으로만 가며 키는 헤더에만 있다(본문·로그·`remote_events`에 없음). 위키 폴더 전체, 페이지 목록, 창 제목, URL(링크 자료는 열지 않고 사용자가 쓴 메모만 보낸다), 입력한 문장은 보내지 않는다. 탭 판단은 `llm.judge_enabled`(기본 꺼짐) 동의가 있어야 하며 도메인·줄인 탭 제목(120자 이하)·공부 주제만 보낸다(경로·쿼리 없음, 저장 안 함). 병합·점검·문제 생성·자동 조정·챗봇·주간 인사이트도 같은 `MessagesRequest`로만 나간다(계획·집계·위키 본문, 창 제목·앱 이름 없음). 근거와 한도: `docs/decision-llm-egress.md`.

계획 이름·과목은 폰과 방에서 보기 위해 서버 미러에 올라가지만 **알림 문구에는 절대 넣지 않는다**(시각과 고정 문장만). 탭 기록·창 제목·URL·타이핑은 이 표 어디에도 없다.

서버 쪽(Edge Function) 규칙: ntfy 서버 주소는 https 공개 호스트 이름만(IP·localhost·내부 이름·포트·경로·인증 정보 거부). 이 검사는 `supabase/functions/_shared/channels.ts`에 있고 데스크톱과 같은 `fixtures/notify/contract.json`으로 시험한다.

## 잠금 중 설정 변경

계획된 공부 진행 중에는 모든 설정이 잠긴다(D-1). 예외는 반출을 줄이는 방향뿐이며 `setting_history`에 `applied_privacy_tighten`으로 남는다: 모의 모드로 전환, 알림 채널 끄기, 비밀 삭제, 로그아웃. 키 입력·로그인·모드를 실제로 되돌리기는 잠긴다(시도는 `rejected_locked`로 기록).
