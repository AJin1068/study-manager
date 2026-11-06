# 5단계 검증 기록

5단계(폰 앱) 전체(S5.0~S5.9)의 검증 문서다. 에이전트 환경에는 Android SDK·NDK·기기, Docker·Supabase CLI·실제 프로젝트, 폰 카메라, GUI, Windows, GitHub 원격이 없다. 그래서 Kotlin은 한 번도 컴파일·실행되지 않았고, 서버 SQL·pgTAP도 실행되지 않았으며, 폰 쪽은 호스트에서 도는 Rust·TS 로직(임대 상태기계, 브리지 계약, 모의 차단기)만 시험되었다. 5단계 완료 기준(공부 시간에 폰이 PC와 함께 잠김)은 V5-10을 사람이 확인하기 전까지 충족된 것으로 표기하지 않는다. 구조 결정은 `docs/decision-phone-architecture.md`.

## 스토리 상태

「완료」는 에이전트가 증명할 수 있는 범위(호스트 시험)까지다. 기기·서버·CI 동작은 어떤 행도 증명하지 않는다.

| 스토리 | 내용 | 상태 | 남은 것 |
|---|---|---|---|
| S5.0 | 모바일 구성 스파이크 | 완료(문서 조사만, 빌드 시도 없음) | 옵션 A(Tauri 모바일)의 타당성은 조사 결론일 뿐 V5-1 |
| S5.1 | `crates/link-protocol`(타입, `lock_decision`, `LeaseHolder`) | 완료(호스트 시험) | Android 타깃 컴파일 V5-1 |
| S5.2 | 서버 스키마·익명 로그인·`claim_pair`·해제 | 구현. **SQL은 실제 Supabase에서 실행된 적 없음**(임시 Postgres 16+pgTAP 대용으로 한 번 통과, FakeBackend 의미와 정적 문자열 시험) | V5-2 |
| S5.3 | PC 신호 발행, 폰 비트 간격(`phone_gap`), 폰 공통 화면 모델 | **부분**: PC 쪽(발행 큐·재시도·간격 행·경고)은 완료. 폰 쪽은 `phone/` 프로젝트가 **작성되었다**: `phone/core`(임대 폴링 드라이버 `PhonePoller`, 익명 로그인·`claim_pair`·`phone_lease`·`phone_beat` REST 어댑터, 짝짓기, 시간표·타이머·진단 모델; 호스트 `cargo test` 통과), `phone/src-tauri`(Tauri 2 모바일 셸: 상태 저장, 폴링 스레드, 명령 8개; 호스트 `cargo check`/`cargo test` 통과), `phone/src`(React 화면 4개: 시간표·타이머·진단·QR 짝짓기). **안드로이드용으로는 한 번도 빌드되지 않았다**(`#[cfg(target_os = "android")]` 가지와 `tauri-plugin-barcode-scanner` 의존은 호스트에서 컴파일되지 않음). 알려진 빈틈: ① 서버가 짝지은 폰에 계획 행을 주는 경로가 없어(폰 권한은 `phone_*` 표뿐) 시간표 화면은 항상 「표시할 시간표 없음」, ② 포그라운드 서비스를 쓰지 않아 폴링 스레드는 앱 프로세스가 죽으면 멈추고(플러그인의 이미 걸린 잠금은 자체 마감까지 유지), ③ 화면 켜짐 횟수를 세는 폰 쪽 코드가 없어 폰은 집어 든 횟수를 보고하지 않는다 | V5-1, V5-3, V5-5 |
| S5.4 | `PhoneBlocker` 포트, `MockBlocker`, Kotlin 소스 | 포트·모의·계약 fixtures 완료. Kotlin 소스(접근성 서비스, 전체 화면 차단 액티비티, 허용 목록과 항상 허용되는 긴급 전화, 서비스 꺼짐 감지, 잠금 중 설정 앱 진입 차단, 부팅 후 복구)는 작성만 됨(**컴파일·실행 안 됨**). 이를 구동하는 곳은 이제 `phone/src-tauri`(`PluginSlot` → `AndroidAccessibilityBlocker`, 플러그인 이벤트는 웹뷰가 `addPluginListener`로 받아 Rust로 전달)이지만 **한 번도 안드로이드에서 실행되지 않았다**. 서비스 꺼짐은 `phone_beat(service_enabled=false)`로 보고되고 서버가 `phone_gap(service_disabled)`로 만든다는 연결은 호스트 시험(가짜 링크·모의 차단기)으로만 확인 | V5-1, V5-4, V5-5 |
| S5.5 | 폰 집어 든 횟수 | 완료(호스트 시험). 점수 감점 없음(통계 표시만, O-23) | 기기 정확도 V5-6 |
| S5.6 | APK QR 카드·CI 워크플로 | 구현(정적 시험만). 워크플로는 한 번도 실행되지 않음 | V5-7 |
| S5.7 | 앱 푸시 알림 채널(`phone_app`) | 계약·모의 시험 완료. 실제 ntfy 구독 없음 | V5-8 |
| S5.8 | iOS Unsupported | 완료(`ScreenTimeBlocker`는 항상 `Unsupported`, Swift 파일 0개, UI 안내 문구) | V5-9(화면) |
| S5.9 | 통합 시험·문서 | 완료(이 문서, `phone/publisher_tests/e2e.rs`, `src/pages/phoneE2e.test.ts`, `fixtures/phone/privacy.static.test.ts`) | — |

체크포인트 메모: 5단계는 한 커밋이다. S5.3·S5.4의 폰 쪽은 코드가 있으나 안드로이드에서 빌드·실행된 적이 없다는 점을 위 표에 적었고, 커밋 메시지 끝에 「(미완: S5.3·S5.4 폰 앱 안드로이드 빌드 미검증)」을 붙이는 것을 리더가 판단한다. AGENTS §1의 현재 단계를 완료로 표기하지 않는다.

## 게이트 명령과 결과

아래 칸은 리더가 실제 실행 결과로 채운다. 이 문서를 쓴 에이전트는 이 환경에 Node가 없어 vitest·tsc·eslint를 돌리지 못했고, `cargo test`는 새 e2e 시험과 `phone::publisher_tests`만 돌렸다(25개 통과).

| 명령 | 위치 | 결과 |
|---|---|---|
| `pnpm tsc --noEmit` | 루트 | 종료 0 |
| `pnpm lint` | 루트 | 종료 0 |
| `pnpm test` (vitest, `supabase/**`·`web/**`·`fixtures/**`·`.github/**`·`phone/**` 포함) | 루트 | 511개 통과 |
| `cargo clippy --all-targets -- -D warnings` | `src-tauri` | 종료 0 |
| `cargo test` | `src-tauri` | 1061개 통과 (+통합 7·4·doc 3) |
| `cargo test` (link-protocol) | `crates/link-protocol` | 14 + 통합 6개 통과 |
| `cargo test` (폰 코어: 55개 시험, 이 에이전트가 실행·통과) | `phone/core` | 55개 통과 |
| `cargo check` / `cargo test` (폰 Tauri 셸, 호스트 타깃만; 안드로이드 타깃은 시도하지 못함) | `phone/src-tauri` | 종료 0 / 10개 통과 |
| `pnpm build:web` | 루트 | 종료 0 |
| Windows 타깃 `cargo check --target x86_64-pc-windows-msvc` | `src-tauri` | 종료 101: 번들 SQLite C 헤더 부재 (검증 안 됨) |
| `cargo fmt --check` | `src-tauri` | 실행 불가: rustfmt 미설치 (검증 안 됨) |

`supabase test db`(`rls_s5_phone.sql`, `rls_s5_notify_phone_app.sql`)는 Docker·Supabase CLI가 없어 게이트에 넣지 못했다(V5-2, V5-8).

## 에이전트가 자동으로 증명한 것

- **임대 상태기계**(`link-protocol`): 상태표 경계 ±1ms, seq 역행·중복 거부, 상한 초과 임대 거부, 폰 벽시계를 바꿔도 판정 불변, 같은 serde fixtures(`fixtures/phone/*.json`)를 크레이트·데스크톱·TS가 읽는다.
- **통합 시나리오**(`src-tauri/src/phone/publisher_tests/e2e.rs`, 실제 타이머 엔진·인메모리 DB·`FakeBackend` 서버·가짜 시계·`BlockerDriver`+`MockBlocker`):
  1. PC 공부 시작 → 서버에 발행 → 폰이 임대 수신 → Engaged(남은 시간은 기간으로 전달, 긴급 전화 허용 역할 포함) → PC 긴급 패스 → 폰 Free(`EmergencyPass`) → 패스 종료 시 재잠금 → 구간 끝에 Free(`RunEnded`).
  2. 서버 단절 중에도 PC 타이머·잠금은 그대로, 폰은 임대가 남은 동안 잠김 유지(만료 1초 전까지 Active), 임대 끝+유예 뒤 `LeaseExpired`로 Free(영원히 잠기지 않음), 서버 복귀 후 중복 Free는 변화 없음.
  3. 공부 중 짝 해제 → 폰이 `PairRevoked`로 Free, 이후 구간 전이는 발행·집행되지 않음, 옛 토큰은 읽기·쓰기 모두 거부, PC의 잠금은 그대로. 이 시험은 서버 API로 직접 해제한다(앱의 `PairingService`는 계획된 공부 중 해제를 `run_locked`로 거절한다).
  4. 허용 목록에 카나리 패키지명을 넣어도 비트·집어 든 횟수·발행·서버 표 어디에도 나가지 않고 장치 쪽 차단기에만 도달한다.
- **UI 로직**(`src/pages/phoneE2e.test.ts`): 같은 시간표를 카드가 어떻게 말하는지(조용함 → 신호 실패 경고 → 폰 침묵 → 서비스 꺼짐 간격 「N분째」·점수 영향 없음 문구 → 긴급 패스 후 경고 없음 → 해제된 짝은 경고 없음), 임대→engage 매핑(locked만, 남은 시간 1..=상한, 상한 초과·0·음수 거부, Free 임대는 engage 아님). **이 환경에서 vitest를 실행하지 못했다.** 리더 게이트 결과로 확인한다.
- **카나리 grep**(`fixtures/phone/privacy.static.test.ts`, 위와 같은 사유로 미실행): 공유 wire fixtures에 금지 키(package·app·usage·history·title·url·domain·path) 없음, 보고·발행 Rust 타입의 `pub` 필드에 금지 이름 없음, `PickupReport`는 `segment_id`·`count`뿐이고 `deny_unknown_fields`, 서버 마이그레이션 코드에 내용 컬럼 없음. 실행 중 payload 카나리는 Rust 쪽(`publisher_tests`, `e2e.rs`, `tests.rs`)이 요청 본문 전체에서 검사한다.
- **개인정보 요약**: 폰이 보고하는 것은 임대 수신 확인(`lease_seq`·`locked`·`service_enabled`)과 구간별 집어 든 횟수뿐이며 앱 이름·패키지명·사용 내역·창 제목·URL 필드가 어떤 payload 타입에도 없다. 허용 앱 목록은 장치 로컬이며 서버에 가지 않는다. 반출 허용 목록 변경점은 `docs/egress-policy.md`의 「폰 (5단계)」.

## 검증 안 됨

### 5단계 신규 (V5-x)

| ID | 스토리 | 항목 | 안 된 이유 | 사람이 하는 한 줄 절차 |
|---|---|---|---|---|
| V5-1 | S5.0, S5.1, S5.3 | Tauri 모바일(옵션 A) 빌드 가능성, `link-protocol`의 Android 타깃 컴파일, **`phone/` 앱 본체의 안드로이드 빌드**(본체 코드는 작성됨: `phone/core`, `phone/src-tauri`, `phone/src`) | Android SDK·NDK·JDK 없음 | JDK 17·SDK 34·NDK r26 설치 후 `cargo build --target aarch64-linux-android`(link-protocol)와 `phone/src-tauri`에서 `pnpm tauri android init`(생성된 `gen/android`에 `phone/plugins/blocker/android`를 플러그인 프로젝트로 연결)와 `pnpm tauri android build`가 APK를 만드는지, `build.rs`의 인라인 플러그인 권한(`blocker:default`)과 `barcode-scanner` 권한이 모바일 빌드에서 해석되는지 확인, 막히면 `decision-phone-architecture.md`의 되돌림 조건 |
| V5-2 | S5.2 | 실제 QR 스캔, 익명 로그인(`signInAnonymously` 허용 설정), 실제 Postgres에서 phone 마이그레이션과 RLS·RPC 권한(`rls_s5_phone.sql`) | 폰 카메라·실제 프로젝트·Docker 없음 | 프로젝트에서 익명 로그인을 켜고 마이그레이션 적용 후 `supabase test db`가 통과하는지, PC QR을 폰으로 찍어 짝이 맺어지고 PC에서 끊으면 폰 읽기가 거부되는지 확인 |
| V5-3 | S5.3 | 실기기에서 임대 수신 지연(폴링 ≤15초+푸시)과 폰 비트 끊김 판정 | 실기기·실서버 필요 | 짝지은 폰으로 공부 구간을 시작해 PC 시작부터 폰 잠금까지 걸린 시간과, 비행기 모드 90초 뒤 PC에 간격 경고가 뜨는지 재기 |
| V5-4 | S5.4 | Kotlin 소스 컴파일, `AllowListTest.kt` 실행 | Android 툴체인 없음(소스 정적 시험만 있음) | `phone/plugins/blocker/android`에서 `./gradlew test assembleDebug` 실행 |
| V5-5 | S5.4 | 접근성 권한 흐름, 차단 화면, 설정 앱 진입 차단, 서비스 끄기 감지(`service_disabled`), 부팅 후 임대 복구, 배터리 최적화·제조사별 종료 | 기기 필요 | 접근성 허용 후 공부 구간에서 허용 밖 앱이 덮이는지, 서비스를 끄면 PC에 경고가 뜨는지, 재부팅 후 남은 시간만큼 다시 잠기는지, 제조사 배터리 설정에서 서비스가 살아 있는지 확인 |
| V5-6 | S5.5 | 기기별 화면 켜짐·잠금 해제 횟수 정확도 | 기기 필요 | 공부 구간 중 폰을 10번 집어 든 뒤 PC 대시보드의 횟수와 비교 |
| V5-7 | S5.6 | `android.yml` 실행, APK 서명·릴리스 업로드, APK 설치(알 수 없는 앱 허용) | GitHub 원격·서명 키·기기 없음 | 저장소 비밀에 서명 키를 넣고 태그를 푸시해 Releases에 APK가 올라오는지, PC 설정의 QR로 폰에서 내려받아 설치되는지 확인 |
| V5-8 | S5.7 | 실제 ntfy 푸시 도착(`phone_app` 채널), 절전 모드 지연, `rls_s5_notify_phone_app.sql` 실행 | 실기기·실서버·Docker 없음 | 채널을 폰 앱으로 바꾸고 PC를 끈 채 계획 시각 5분 뒤 폰 알림이 오는지 확인, `supabase test db` 실행 |
| V5-9 | S5.8 외 | 폰 짝짓기 카드·APK 설치 카드·폰 연결 경고·iOS 「Apple 승인 후 지원」 안내의 실제 렌더 | GUI 실행 불가 | 설정 → 폰 카드를 라이트·다크로 열어 문구가 겹치거나 잘리지 않는지, iOS 안내가 보이는지 확인 |
| V5-10 | 전체 | **5단계 완료 기준**: 공부 시간에 폰이 PC와 함께 잠김(실제 PC+실제 폰+실제 서버, 긴급 패스·서버 단절·짝 해제 포함) | 위 항목이 모두 필요 | V5-1~V5-8을 끝낸 뒤 공부 구간 하나에서 시작 → 폰 잠김 → 긴급 패스 → 풀림 → 와이파이 끊고 임대 만료 → 짝 해제를 순서대로 확인 |

### 이전 단계에서 이월되어 아직 열린 항목 (참조)

각 항목의 이유와 사람이 하는 절차는 원 문서에 있다. 여기서는 아래 ID로만 가리킨다.

| 문서 | 열린 항목 | 묶음 요약 |
|---|---|---|
| `docs/stage-1-5-verification.md` 총괄표(1-1~1-4 포함) | #1~#47 | Windows 전체(#1, #2, #17, #33, #37), GUI·트레이·메뉴바(#3, #5, #7, #11), 실제 절전·깨어남(#2, #4, #13), 자동 실행·재로그인(#6, #14, #21), CI 원격 실행(#9), 알림 표시(#10, #23, #39), UI 클릭 동작(#8, #12, #15, #22, #31, #32, #43, #44), 실제 종료 동작(#16, #18, #19, #20), Chrome 확장·네이티브 메시징(#24~#30, #34, #35), 권한·센서·단축키(#36~#39), 실사용 정확도·밸런스(#35, #40~#42, #45), 그림 자산(#46), 1단계 완료 기준 일주일 실사용(#47) |
| `docs/stage-1-4-transport-decision.md` 「Not verified」 | Chrome 확장·`connectNative`·서비스 워커 수명·Windows named pipe·Edge/Brave 경로·확장 ID | 위 #24~#29, #33, #34와 같은 항목 |
| `docs/stage-2-verification.md` | V2-1~V2-12 | 실제 ntfy·Telegram 5분 도착(V2-1), 키체인(V2-2), 연동 화면(V2-3), 실제 Supabase RPC·OTP·마이그레이션·RLS(V2-4, V2-5, V2-8), 수면·깨어남(V2-6), Edge Function·`pg_cron`(V2-7), 외부 .ics(V2-9), ntfy QR(V2-10), 문구 체감(V2-11), 시간대(V2-12) |
| `docs/stage-3-verification.md` | V3-1~V3-13 | 실제 모델 품질·모델명·주입(V3-1, V3-2, V3-4, V3-7, V3-13), PDF 미구현(V3-3), 조기 해제 관문·복습 카드 GUI(V3-5, V3-6, V3-9, V3-10), 맥 창 제목 권한(V3-8), 키체인 Live(V3-11), Windows 위키(V3-12) |
| `docs/stage-4-verification.md` | V4-1~V4-11 | 실제 Postgres RLS·마이그레이션(V4-1, V4-9), 두 계정 방 흐름(V4-2), 그림 자산(V4-3), 공유 위키 합본 품질(V4-4), 웹 호스팅·Realtime 소켓 미구현(V4-5), 딥링크 QR(V4-6), 캐릭터 이름 품질(V4-7), 방·상점 GUI(V4-8), Windows(V4-10), 시계 오차·날짜 경계(V4-11) |

겹치는 환경 한계: 서버 SQL 실행은 V2-8·V4-1·V4-9·V5-2·V5-8을 한 번의 `supabase db push`+`supabase test db`로 같이 확인할 수 있다. Windows는 #1·V3-12·V4-10에 이어 5단계 코드(`phone/`, `crates/link-protocol`)도 같은 Windows `cargo check/test`에 포함된다(위 게이트의 Windows 행).

## 알려진 잔여 위험

- **폰 잠금은 OS가 허용하는 만큼만 약하다**(PLAN 7번: 완전한 먹통·삭제 방지는 OS가 허용하지 않음). 접근성 서비스를 끄거나 앱을 강제 종료·삭제하면 잠금이 풀린다. 대응은 **감지와 기록**이다: 서비스 꺼짐은 `phone_gap(service_disabled)`로 남고 PC가 경고하며 집중 통계의 「폰 미연결 시간」에 반영된다(점수 감점 없음). 삭제·강제 종료는 폰 비트 끊김(`unreachable`)으로만 보인다. 설정 앱 진입 차단은 Kotlin 소스에만 있고 V5-5 전까지 동작을 모른다.
- **임대 의미**(오프라인에서 일부러 약하게 풀리는 방향이 아니라 일부러 오래 잠기는 방향의 한계):
  - 임대는 구간 끝+유예 60초까지 오프라인에서도 잠금을 유지한다(fail-closed). 상한은 3시간+60초이며 서버·클라이언트 모두 거부한다.
  - PC가 꺼지거나 서버가 죽어도 폰은 영원히 잠기지 않는다. 임대가 끝나면 반드시 Free다(`LeaseExpired`).
  - 반대로, 폰이 오프라인일 때 PC가 긴급 패스·건너뛰기·조기 해제·짝 해제를 해도 폰은 **임대가 끝날 때까지 잠겨 있을 수 있다**(최대 한 구간+유예). UI가 고지한다. 긴급 전화는 항상 허용된다.
  - 임대는 폰의 단조 시계 경과와 서버 `server_now` 오프셋으로 판정하므로 폰 벽시계 변경은 효과가 없다. 단 재부팅 뒤에는 단조 시계가 초기화되어, Kotlin 쪽 복구(`LockController`)가 같은 부팅이면 단조 마감을, 부팅이 바뀌었으면 저장해 둔 벽시계 마감과 남은 시간 중 작은 값을 쓴다. 즉 재부팅 직후 구간에서는 폰 벽시계 조작이 잠금을 줄일 수 있다(코드 읽기만 했고 실행되지 않았다, V5-5).
  - 짝 해제는 서버 RLS가 즉시 접근을 거부하므로 폰은 다음 확인(최대 15초) 때 `PairRevoked`로 풀린다. 앱은 계획된 공부 중 PC에서의 해제를 거절하지만(`run_locked`), 서버에 직접 접근하는 소유자의 해제는 막지 못한다.
- **짝 토큰 탈취**: 폰 계정은 익명이고 접근 범위는 이 소유자의 `phone_*` 행뿐이다. 코드는 1회용 5분, 소유자 해제 시 JWT가 남아 있어도 RLS로 거부된다. 실제 Postgres에서 확인되지 않았다(V5-2).
- **폰 앱 본체는 작성되었지만 검증되지 않았다**: `phone/core`(호스트 시험 통과)가 `src-tauri/src/phone/`의 허용 목록·차단기 포트·브리지 파일을 **복사본**으로 갖는다(폰이 `src-tauri`를 의존할 수 없기 때문). 복사본의 일치는 같은 `blocker-bridge.json`을 두 쪽 시험이 읽는 것으로만 지켜지며 한쪽만 고치면 어긋날 수 있다. 안드로이드 빌드, 접근성 서비스 실제 동작, 포그라운드 서비스 부재(앱 프로세스가 죽으면 폴링 중단), 계획 행을 폰에 주는 서버 경로 부재(시간표 화면은 비어 있음), 폰의 집어 든 횟수 계수 코드 부재는 모두 열려 있다(V5-1, V5-4, V5-5, V5-6). 리프레시 토큰은 앱 전용 저장소의 JSON 파일에 평문으로 있어 루팅된 폰에서는 읽힌다. 폰 앱에서 「짝 끊기」는 잠금 중에는 거절되지만 앱 데이터 삭제는 막지 못한다.
- **iOS**: 항상 `Unsupported`이며 Apple Screen Time 승인 전에는 폰 잠금이 없다. 안내 문구만 있다.
- **집어 든 횟수**: 통계 표시만 하고 점수에는 영향이 없다(O-23, 실사용 후 결정).
- **익명(폰) 계정의 소유자 RPC**: 폰은 익명 계정이라 예전에는 `create_pair_code` 말고는 방 만들기·프로필 공개·선물 같은 소유자 RPC를 막는 장치가 없었다. `20260601000700_s5_anonymous_guard.sql`이 폰 쪽 RPC(`claim_pair`, `phone_lease`, `phone_beat`, `report_phone_pickups`)와 `get_public_profile`을 뺀 모든 소유자 RPC 맨 앞에 `anonymous_not_allowed` 거부를 넣는다(목록은 `supabase/tests/anonymous_guard.static.test.ts`가 RPC 계약과 대조). 이 마이그레이션과 `rls_s5_anonymous_guard.sql`, 수정된 `rls_s4_profile.sql`은 Supabase가 아닌 **임시 Postgres 16(auth 스키마를 흉내 낸 스텁, pgTAP 대신 최소 대용 함수)에서 한 번 실행되어 통과**했다. 실제 Supabase 스택·`supabase test db`·실제 익명 로그인 JWT는 아직이다(V5-2).
- **서버 SQL 미실행**: 위 한 번의 스텁 실행을 빼면 FakeBackend와 정적 문자열 시험뿐이며, 이는 Postgres RLS의 증명이 아니다.

## 다음 단계로 넘기는 항목

- `phone/` 앱의 Android 빌드 검증과 남은 빈틈: 포그라운드 서비스, 계획 행을 폰에 주는 서버 경로, 화면 켜짐 계수(V5-1, V5-4, V5-5, V5-6).
- 실제 서버·기기에서의 종단 확인(V5-2, V5-3, V5-10).
- 집어 든 횟수를 점수에 반영할지(O-23).
- 이전 단계 항목은 위 참조표의 원 문서에서 계속 추적한다.
