## Workflow
- context 파일 읽고 **다음 목표** 확인 후 수행
- 계획 (두 가지 경로)
  - **plan 모드**: `/plan`으로 진입. `~/.claude/plans/`에 작성
  - **contexts 계획**: 계획 요청 시 `.contexts/`에 작성, context 파일에 Link
- 커밋 완료 후 context 파일 업데이트. 푸시는 요청할 때만
  
## 현재 상태

### 워치리스트 파이프라인
- 2테이블: pending(사냥 버퍼) + candidates(judged/blacklisted)
- 사냥(30분) / 가죽 작업(하루 2회) 독립 사이클, 테이블 소유권 분리
- 세계관: 모하비 게코 사냥꾼 (docs/glossary.md)

### 명령어 체계
- gecko(pending) / pelt(judged) 개념 분리
- `/w hunt`, `/w ls [gecko|pelt]`, `/w clear gecko|pelt|bl`

### 이력
- 3/17 파이프라인, 3/18 세계관, 3/19 문구, 3/20 hunt_count
- 3/21 사이클 분리 + 테이블 분리 + 명령어 개편(gecko/pelt)
- 3/22 /w ls 상태별 메시지 + add_blacklist reason 덮어쓰기 버그 수정(verdict로 분리) + 배포

## 다음 목표

(취소됨)

## 핵심 메모
- 런타임: tokio current_thread + LocalSet, Actor 패턴 (mpsc/oneshot), 뮤텍스 프리
- 저장소: SQLite WAL, thread_local RefCell<Connection>
- API: 한투 OpenAPI (KIS) + Google AI Studio (Gemini/Gemma) + Telegram (teloxide)
- 배포: OCI 인스턴스, systemd user 서비스
