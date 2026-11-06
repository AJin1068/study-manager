# 결정: 폰 앱 구성 (S5.0)

상태: 결정됨(문서 조사). 이 환경에는 Android SDK·NDK·기기가 없어 아래 모든 빌드 주장은 **검증 안 됨**(V5-1)이며, 사람이 `docs/stage-5-verification.md`의 절차로 확인한다.

## 문제

공부 시간에 폰이 PC와 함께 잠겨야 한다. 데스크톱은 Tauri 2 + React이다. 폰 앱을 어떻게 만들지, 데스크톱 코드와 무엇을 공유할지 정한다.

## 변하지 않는 부분 (어느 옵션이든)

- `crates/link-protocol`: 직렬화 타입(PairingOffer, LockLease, PhoneBeat, PickupReport)과 순수 임대 상태기계 `lock_decision`. 독립 크레이트(자체 Cargo.toml·lock)이며 루트 Cargo 워크스페이스는 만들지 않는다. 이유: CI가 `src-tauri`에서 cargo를 돌리고, 루트 `Cargo.toml`이 없으며, `src-tauri`에는 macOS 전용 의존(objc2, keyring 백엔드)이 있어 폰 타깃과 한 워크스페이스로 묶으면 해석이 깨진다. `src-tauri`는 경로 의존(`link-protocol = { path = "../crates/link-protocol" }`)으로만 쓴다.
- 공유 JSON fixtures(`fixtures/phone/*.json`): 크레이트 시험, 데스크톱 시험, 폰 프로젝트가 같은 바이트를 읽는다. 폰이 Kotlin이면 같은 fixtures로 Kotlin 쪽 시험을 쓴다.
- 서버 계약: `fixtures/phone/rpc-contract.json`, `fixtures/phone/rls-matrix.json`. 폰은 Supabase 익명 로그인으로 자기 `auth.uid()`를 얻고 `claim_pair`로 PC와 묶인다.

## 옵션 A — Tauri 2 모바일 독립 프로젝트 (`phone/`) (기본)

- 구성: `phone/`에 자체 Tauri 모바일 프로젝트. 화면은 React 컴포넌트를 `src/`에서 재사용(시간표, 타이머, 임대 진단). 접근성 서비스·차단 화면은 Tauri 플러그인(`phone/plugins/blocker/android/`의 Kotlin)로 구현하고 Rust 쪽 `PhoneBlocker` 포트가 브리지한다.
- 장점: PLAN 기술 스택과 일치. `link-protocol`을 Rust 그대로 폰에서 링크하므로 판정 코드가 PC와 폰에서 한 벌. 화면 재사용.
- 단점: Android 툴체인(SDK, NDK, Java 17, `cargo-ndk`/Tauri 모바일 초기화)이 필요하고 Tauri 모바일 플러그인 API는 데스크톱보다 성숙도가 낮다. 접근성 서비스는 어차피 Kotlin이다. 포그라운드 서비스를 WebView 수명과 분리해야 한다(WebView가 죽어도 잠금이 유지되어야 함).
- 필요한 것(조사): Android SDK 플랫폼 34 이상, NDK r26 이상, JDK 17, Rust 타깃 `aarch64-linux-android`(+`armv7`, `x86_64`), `minSdk` 26 이상(접근성·포그라운드 서비스 유형 요구).
- 데스크톱과의 결합 지점: (1) `link-protocol` 경로 의존, (2) fixtures, (3) 서버 RPC 계약. 그 밖의 `src-tauri` 모듈에는 의존하지 않는다(macOS 전용 의존 때문).
- 의존성 충돌 위험: 폰 프로젝트가 `src-tauri`를 의존하면 objc2/keyring이 Android 타깃에서 깨진다 → 금지. 폰은 `link-protocol`만 의존한다.

## 옵션 B — 순수 Kotlin 앱 + fixtures 공유

- 구성: Kotlin(Compose) 앱. `link-protocol`의 판정을 Kotlin으로 다시 구현하고 같은 fixtures로 맞춘다.
- 장점: 접근성 서비스·포그라운드 서비스·부팅 복구가 모두 네이티브라 제조사 대응이 쉽다. WebView 없음.
- 단점: 판정 로직이 두 벌(Rust, Kotlin)이다. 드리프트는 fixtures 시험으로만 막는다. 화면 재사용 불가.

## 결정

옵션 A로 진행한다. `link-protocol`은 Rust 크레이트로 유지하고, 차단기와 서비스 수명은 Kotlin 플러그인에 둔다. 시간 상자 안에서 조사만으로 확신하지 못한 항목은 V5-1로 남긴다.

## 되돌림 조건

다음 중 하나가 사람 검증에서 확인되면 옵션 B로 전환하거나 폰 앱을 별도 저장소로 분리한다(PLAN 리스크 항목).

1. Tauri 모바일 프로젝트가 위 SDK/NDK 조합으로 APK를 만들지 못한다.
2. WebView가 종료되는 상황(메모리 압박, 절전)에서 포그라운드 서비스가 임대를 유지하지 못한다.
3. 제조사 배터리 최적화로 서비스가 죽고 플러그인 구조로는 복구할 수 없다.

옵션 B로 바꿔도 바뀌는 것은 `phone/` 디렉터리뿐이다. 서버 스키마, `link-protocol` 타입, fixtures, PC 쪽 코드는 그대로다.

## 한계 (정직한 표기)

- OS는 접근성 서비스 끄기·앱 삭제를 완전히 막지 못한다. 대응은 서비스 꺼짐 감지 → PC 경고 + 집중 통계 반영이며 완전 방지가 아니다(PLAN 7).
- iPhone: 스크린 타임 인증이 Apple 승인을 요구하므로 `BlockerStatus::Unsupported`만 반환한다. Swift 코드는 두지 않는다(S5.8).
- 임대는 만료되면 반드시 풀린다. PC 해제 직후 폰이 오프라인이면 만료까지 잠김이 남을 수 있다(긴급 전화는 항상 허용).
