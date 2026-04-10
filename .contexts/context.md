## Workflow
- 세션 시작 시 **현재 상태** 파악, **다음 목표**를 기준으로 작업 수행
- 계획 요청 시 `.contexts/`에 작성, context 파일에 Link
- 커밋 완료 후 context 파일 업데이트. 푸시는 요청 시에만 수행

## 현재 상태
- [2026/04/11] /port list 캐시 전략 개선
  - age ≤ 1분: 캐시 직접 사용, 1분~5분: API 시도 후 실패시 캐시 폴백
  - 유닛테스트 169개 통과
- [2026/04/11] port ex 명령어에 Google Sheet 가이드 문구 추가
  - CSV 파일 전송 후 'Google Sheet에서 import' 안내 문구 추가
  - 유닛테스트 169개 통과, OCI 배포 완료
- [2026/03/22] /w ls 상태별 메시지 + add_blacklist reason 덮어쓰기 버그 수정 + 배포
- [2026/03/21] 사이클 분리 + 테이블 분리 + 명령어 개편(gecko/pelt)
- [2026/03/20] hunt_count 가중치 추가
- [2026/03/19] 워치리스트 문구 정리
- [2026/03/18] 세계관 적용 (모하비 게코 사냥꾼)
- [2026/03/17] 파이프라인 구현 (pending + candidates 테이블)

## 다음 목표
- [x] [.contexts/plan-cache-strategy.md](.contexts/plan-cache-strategy.md) - `/port list` 캐시 strategy 개선
- [ ] [.contexts/plan-request-batching.md](.contexts/plan-request-batching.md) - API 요청 배치 처리 (threshold: 100개+)

## 핵심 메모
- **GROUP, OTHER 에게 권한 없는 파일 접근 금지**
- 런타임: tokio current_thread + LocalSet, Actor 패턴 (mpsc/oneshot), 뮤텍스 프리
- 저장소: SQLite WAL, thread_local RefCell<Connection>
- API: 한투 OpenAPI (KIS) + Google AI Studio (Gemini/Gemma) + Telegram (teloxide)
- 배포: OCI 인스턴스, systemd user 서비스