# pending 테이블 분리 계획

## 프레임

- **사냥 = 생산자**: pending 버퍼에 쌓기만 함
- **가죽 작업 = 소비자**: 버퍼를 읽어서 정제 → candidates 내에서 judged/blacklisted 관리

테이블 2개. pending = 사냥 도메인, candidates = 가죽 도메인.

## 테이블 설계

```sql
-- 사냥 전용 버퍼 (hunt CRUD, judge READ only)
CREATE TABLE pending (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ticker TEXT NOT NULL UNIQUE,
    market TEXT NOT NULL DEFAULT '',
    name TEXT NOT NULL DEFAULT '',
    sector TEXT NOT NULL DEFAULT '',
    reason TEXT NOT NULL DEFAULT '',
    hunt_score REAL,
    hunt_count INTEGER NOT NULL DEFAULT 1,
    prompt_id INTEGER,
    created_at TEXT NOT NULL
);

-- 가죽 = 감정 완료 (judge CRUD)
CREATE TABLE candidates (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ticker TEXT NOT NULL UNIQUE,
    market TEXT NOT NULL DEFAULT '',
    name TEXT NOT NULL DEFAULT '',
    sector TEXT NOT NULL DEFAULT '',
    reason TEXT NOT NULL DEFAULT '',
    hunt_score REAL,
    hunt_count INTEGER NOT NULL DEFAULT 1,
    score REAL,
    verdict TEXT,
    detail_text TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'judged',  -- judged | blacklisted
    strike_count INTEGER NOT NULL DEFAULT 0,
    judged_at TEXT,
    created_at TEXT NOT NULL
);
```

blacklist 별도 테이블 제거. candidates.status='blacklisted' + strike_count로 흡수.

## 소유권

| 테이블 | 사냥 사이클 | 가죽 작업 |
|--------|-----------|----------|
| pending | **CRUD** | READ only |
| candidates | READ (BL 체크) | **CRUD** |

## 사이클별 흐름

### 사냥 사이클 (`run_hunt`)

```
1. cleanup: DELETE FROM pending
     WHERE ticker IN (SELECT ticker FROM candidates)
2. cleanup_old_data(retention_days)
3. gemini::hunt() → BL 체크(candidates) → INSERT pending (기존이면 count++)
```

### 가죽 작업 (`run_evaluate`)

```
1. 부활: candidates WHERE status='blacklisted'
     AND score IS NOT NULL AND score >= min×0.9 AND strike_count < 3
     → INSERT pending (hunt_count 이관), DELETE FROM candidates

2. 수집 pending: (BL 스킵: WHERE ticker IN candidates AND status='blacklisted')
     성공 → ready Vec
     실패 → INSERT candidates (status='blacklisted', score=NULL) = 영구 BL

3. 수집 candidates (judged 재감정):
     성공 → ready Vec
     실패 → 스킵 (기존 score 유지)

4. 감정 (배치):
     pending 통과 → INSERT/UPDATE candidates (judged, count 합산)
     pending 미달 → INSERT candidates (blacklisted, strike+1)
     candidates 재감정 미달 → UPDATE status='blacklisted', strike+1
     미매칭 → INSERT/UPDATE candidates (blacklisted)

5. 도태: 상위 N 외 → UPDATE status='blacklisted', strike+1
```

pending은 건드리지 않음. 다음 사냥 사이클에서 cleanup.

## hunt_count 흐름

```
사냥:
  pending에 있음 → pending.hunt_count++
  candidates에 있음(judged) → pending에 INSERT (count=1)
  candidates에 있음(BL) → 사냥 BL 필터 스킵

가죽 작업 (pending → candidates 이관):
  candidates에 이미 있음 → candidates.hunt_count += pending.hunt_count
  candidates에 없음 → INSERT candidates (pending.hunt_count 이관)
```

## 영구 BL

기존 방식 그대로: **score=NULL → revive 조건 `score IS NOT NULL` 탈락**.

| BL 원인 | score | revive |
|---------|-------|--------|
| 감정 미달 | 있음 | ✅ (score≥min×0.9, strike<3) |
| 도태 | 있음 | ✅ |
| 수집 실패 | NULL | ❌ 영구 |
| 수동 척살 | NULL | ❌ 영구 |

## 기존 대비 변경 요약

| 항목 | 현재 | 변경 후 |
|------|------|---------|
| 테이블 | candidates(3상태) + blacklist | pending + candidates(2상태) |
| pending 관리 | candidates.status='pending' | 별도 테이블, 사냥 소유 |
| BL 관리 | blacklist 별도 테이블 | candidates.status='blacklisted' |
| strike_count | blacklist.strike_count | candidates.strike_count |
| 영구 BL | score=NULL | score=NULL (동일) |
| status 값 | pending/judged/blacklisted | judged/blacklisted (pending은 테이블) |

## 코드 변경

| 파일 | 변경 |
|------|------|
| models.rs | CandidateStatus: Pending 제거 → Judged/Blacklisted. PendingEntry 추가 |
| db.rs | 테이블 생성, pending CRUD, blacklist 함수 → candidates 쿼리로 전환 |
| pipeline.rs | run_hunt: pending cleanup + insert. run_evaluate: pending READ + candidates CRUD |
| commands.rs | /w pending → pending 테이블, /w bl → candidates WHERE blacklisted, BL 체크 변경 |
| scheduler.rs | 변경 없음 |
| gemini.rs | 변경 없음 |
| architecture.md, README.md | 테이블 구조 업데이트 |

## 구현 순서

| 단계 | 작업 |
|------|------|
| 1 | models.rs: PendingEntry 추가, CandidateStatus에서 Pending 제거 |
| 2 | db.rs: pending 테이블 + candidates 변경 (blacklist 흡수) |
| 3 | pipeline.rs: run_hunt (cleanup + insert), run_evaluate (소유권 준수) |
| 4 | commands.rs: 쿼리 변경 |
| 5 | 문서 동기화 |
| 6 | 빌드 + 테스트 |

한 커밋. 데이터 날려도 되므로 마이그레이션 불필요.
