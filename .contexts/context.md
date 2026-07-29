## Workflow
- 세션 시작 시 **현재 상태** 파악, **다음 목표**를 기준으로 작업 수행
- 계획 요청 시 `.contexts/`에 작성, context 파일에 Link
- 커밋 완료 후 context 파일 업데이트. 푸시는 요청 시에만 수행

## 현재 상태
- [2026/07/29] 스케줄러 `biased;` 추가 + 브랜치 순서 재배열
  - `select!` 브랜치 순서: signal > eval > hunt > trigger (기존: signal > hunt > trigger > eval)
  - 동시 발동 시 랜덤 선택 방지, 시그널 체크 우선순위 보장
  - `Arc<tokio::sync::Mutex>` → `Rc<RefCell>` 교체 시도했으나 dptree `Send + Sync` 제약으로 불가 확인
  - AGENTS.md / docs/architecture.md에 "Mutex 금지 — 단, dptree 제약 예외" 명시
- [2026/05/19] Gemini 사냥 JSON 파싱 실패 해결 → `responseSchema` 강제 도입
  - 증상: 24시간 안 6건 hunt parse_error. 전부 `gemini-3.1-flash-lite`
  - 패턴 1: 모델이 캐릭터 몰입해 reason 닫고 따옴표 밖에 평문 욕설 끼움
  - 패턴 2: stop token 못 만들고 닫는 괄호 `]`/`}` hallucinate
  - 원인: `responseMimeType=application/json`은 JSON 형식만 요구할 뿐 구조 강제 못 함
  - 대응: `call_llm` 시그니처에 `response_schema: Option<&Value>` 추가. hunt는 `Vec<HuntResult>`, judge는 `Vec<JudgeResult>` OpenAPI 3.0 subset schema 주입. `required` + `propertyOrdering` 명시
  - 프롬프트의 인라인 schema 예시 제거 (중복, 충돌 위험)
  - 검증: 배포 후 첫 사이클 동일 모델로 30개 후보 성공 수집
- [2026/05/15] 가죽 0 사태 → 모델 교체 + 운영 안정화 패키지
  - **Gemma 사망**: gemma-3 deprecated → gemma-4는 mimeType=JSON 미지원으로 timeout/500
  - judge_models: gemma-3-27b-it → `["gemini-3.1-flash-lite", "gemini-2.5-flash-lite"]`
  - hunt_models 정비: 3.1 우선 (RPD 500), 2.5 시리즈는 폴백 (RPD 20)
  - max_judge_calls_per_day: 14400 → 50 (실 사용 4~6회의 10배)
  - **한투 BL 3진 아웃화**: pending.strike_count 추가. 단발 실패→영구 BL 정책 폐기 (1.5일 누적해야 BL)
  - **묵음 폴백 제거**: `unwrap_or_default()` → 진짜 parse_error. prompt_history에 박힘
  - **LLM 60초 timeout** 추가 (gemini.rs:call_llm). hang 방지
  - **`/w eval` 수동 명령** 추가 (자동 사이클 KST 02/14 외 즉시 트리거)
  - **프롬프트 markdown 펜스 제거** — mimeType=JSON과 충돌해 모델이 끝에서 hallucinate. raw schema만 명시
- [2026/04/11] price=0 문제 해결 / API Actor 파이프라이닝 + 캐시
- [2026/03/22] /w ls 상태별 메시지 + add_blacklist reason 버그 수정
- [2026/03/21] 사이클 분리 + 테이블 분리 + 명령어 개편(gecko/pelt)
- [2026/03/17~20] 파이프라인 구현, 세계관, 문구, hunt_count

## 다음 목표
- 운영 모니터링 — `biased` 적용 후 select! 우선순위 정상 동작 확인
- 운영 모니터링 — schema 강제 효과 확인 (24시간 parse_error 카운트 0 유지)
- 운영 모니터링 — 가죽 안정 확보 확인
- (선택) API key를 URL params에서 header로 옮기기 (로그 노출 방지)

## 핵심 메모
- **GROUP, OTHER 에게 권한 없는 파일 접근 금지**
- 런타임: tokio current_thread + LocalSet, Actor 패턴 (mpsc/oneshot), 뮤텍스 프리
- 저장소: SQLite WAL, thread_local RefCell<Connection>
- API: 한투 OpenAPI (KIS) + Google AI Studio (Gemini/Gemma) + Telegram (teloxide)
- 배포: OCI 인스턴스, systemd user 서비스