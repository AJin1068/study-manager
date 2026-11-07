# 작업 인계 (HANDOFF)

다른 PC에서 이어서 개발할 때 이 문서부터 읽는다. 마지막 갱신: 2026-11-06.

## 현재 상태
- 로드맵 1~5단계 구현 완료 (docs/PLAN.md 기준, 기능 1~17).
  | 단계 | 커밋 |
  |---|---|
  | 1단계 PC 차단기 MVP | 25be0c4, 563522d |
  | 2단계 서버 연결·놓친 계획 알림 | 057d353 |
  | 3단계 LLM Wiki | 1ae11ee |
  | 4단계 스터디방 | 284755e |
  | 5단계 폰 앱 | 6c78004, 029a32c |
- 외부 계정이 필요한 부분(Supabase, 텔레그램/ntfy, Anthropic API 키, Android SDK)은 인터페이스 + mock으로 구현되어 있다. 설정에 키를 넣으면 실제로 동작하는 구조.
- 아이폰 Screen Time은 인터페이스와 '미지원' 결과까지만.
- **아직 안 한 것: 사람 검증.** 각 단계의 `docs/stage-*-verification.md`에 "검증 안 됨" 항목과 한 줄 확인 절차가 있다. 1단계 전체는 `docs/stage-1-5-verification.md`의 "1단계 검증 안 됨 총괄표"(47개). 사람·macOS 확인 항목은 `docs/MANUAL-CHECKLIST.md`에 모았고, Windows PC 검증 결과(2026-11-07)는 각 단계 문서(`docs/stage-*-verification.md`)의 "Windows PC 검증 결과" 절에 있다.

## 새 PC 준비
1. `git clone https://github.com/AJin1068/study-manager.git`
2. Node LTS, pnpm(`npm i -g pnpm`), Rust(rustup), Tauri 2 사전 요구사항(macOS: Xcode Command Line Tools / Windows: WebView2, MSVC Build Tools)
3. `pnpm install --frozen-lockfile` 후 `pnpm tauri dev`
4. 폰 앱(5단계)은 Android Studio + SDK 필요. 구조는 `docs/decision-phone-architecture.md`.
5. gjc 설치 후 프로젝트 폴더에서 `gjc` 실행. 모델은 Sonnet 권장(Haiku는 ralplan 승인 버튼을 자유 입력으로 잘못 읽는 문제가 있었음).

## gjc 설정 (.gjc/는 git에 안 올라가므로 새 PC에서 다시 만든다)
`.gjc/config.yml`:
```yaml
gjc:
  ralplan:
    maxIterations: 8
    autoHandoff: ultragoal
```
- gjc 에이전트는 `.gjc/` 폴더에 직접 쓸 수 없으니 사람이 만든다.
- 이전 PC의 gjc 세션 기록(ultragoal ledger)은 넘어오지 않는다. 새 작업은 `/skill:ralplan ...`으로 새로 시작하면 된다.

## 다음에 할 일 (추천 순서)
1. 핵심 검증: 앱 실행 → 캘린더 계획 → 공부 시작 시 차단·잠금 동작 → 대시보드 기록 확인.
2. 실제 키 연결: Supabase 프로젝트 생성(`supabase/` 마이그레이션 적용), 텔레그램 봇 또는 ntfy, Anthropic API 키.
3. 며칠 실사용 후 불편한 점을 "고도화"로 정리.
4. 사람 검증 항목을 단계별로 처리하고 verification 문서 갱신.

## 알려진 메모
- 앱은 메뉴바 앱이라 처음 켜면 창이 숨겨져 있다. 메뉴바 아이콘 → "설정 열기". Dock 아이콘 재클릭으로는 창이 안 뜬다(미수정).
- gjc에서 Ctrl+C는 입력 지우기, 종료는 `/exit` 또는 Ctrl+D. 같은 세션 이어서 열기는 `gjc -c`.
