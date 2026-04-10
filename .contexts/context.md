## Workflow
- 세션 시작 시 **현재 상태** 파악, **다음 목표**를 기준으로 작업 수행
- 계획 요청 시 `.contexts/`에 작성, context 파일에 Link
- 커밋 완료 후 context 파일 업데이트. 푸시는 요청 시에만 수행

## 현재 상태
- [2026/04/11] API Actor 파이프라이닝 (FuturesUnordered) + 캐시 전략 개선
  - /port list: 26.8초 → 2.7초 (파이프라인) → 0.0초 (캐시 히트)
  - Actor: send_with_retry free function, API 함수 시그니처 변경 (ctx → client,headers)
  - 캐시: age ≤ 1분 캐시 사용, 1분~5분 API 시도 후 실패시 폴백
- [2026/04/11] port ex 명령어에 Google Sheet 가이드 문구 추가
- [2026/03/22] /w ls 상태별 메시지 + add_blacklist reason 버그 수정
- [2026/03/21] 사이클 분리 + 테이블 분리 + 명령어 개편(gecko/pelt)
- [2026/03/17~20] 파이프라인 구현, 세계관, 문구, hunt_count

## 다음 목표
- 운영 데이터 확인

## 핵심 메모
- **GROUP, OTHER 에게 권한 없는 파일 접근 금지**
- 런타임: tokio current_thread + LocalSet, Actor 패턴 (mpsc/oneshot), 뮤텍스 프리
- 저장소: SQLite WAL, thread_local RefCell<Connection>
- API: 한투 OpenAPI (KIS) + Google AI Studio (Gemini/Gemma) + Telegram (teloxide)
- 배포: OCI 인스턴스, systemd user 서비스