# fix: hunt_count 보너스 실효성 개선

## 배경
hunt_count 보너스의 취지: 기존 양피(judged)가 80점대로 자리잡고 있을 때, 반복 추천되는 신규가 누적 보너스로 경계선을 밀어내고 진입하는 것.

### 현재 문제 3가지

**1. count가 안 쌓임**
- pending에서만 `hunt_count + 1`, 사이클이 pending→collected→judged 원샷이라 거의 1 고정
- judged/collected/blacklisted 상태에서 재추천 시 count 무시

**2. score에 보너스가 녹아들어감**
- 사냥 사이클: `score = hunt_s × W + judge_s × (1-W) + hunt_bonus` → 보너스 포함 저장
- 재평가: `score = hunt_s × W + judge_s × (1-W)` → 보너스 없이 저장
- 같은 score 필드에 보너스 포함/미포함이 혼재 → cull 비교 시 불공평

**3. 보너스가 척살 판정에도 영향**
- `final_score < min_score`로 척살 판정 → 보너스 덕에 척살 면하는 케이스 발생
- 보너스 취지는 도태(cull)에서 기존 양피를 밀어내는 것이지, 실력 미달을 구제하는 게 아님

## 해결 방안

### 핵심 원칙
- **score = 기본 점수만 저장** (보너스 미포함, 사냥/재평가 동일)
- **척살 판정 = 기본 점수 vs min_score** (순수 실력)
- **도태(cull) = score + 동적 보너스** (사냥 사이클에서만)
- **count = 상태 불문 항상 +1** (사냥에서 추천될 때마다)

### 시뮬레이션 (max_survivors=50, min_score=60, weight=3.0)

기존 양피 최하위 81점(count=1), 신규 A 기본 78점이 매 사이클 추천:

| 사이클 | A count | A effective         | 기존 effective      | 결과    |
|--------|---------|---------------------|---------------------|---------|
| 1      | 1       | 78 + 2.08 = 80.08   | 81 + 2.08 = 83.08   | A 탈락  |
| 2      | 2       | 78 + 3.30 = 81.30   | 81 + 2.08 = 83.08   | A 탈락  |
| 3      | 3       | 78 + 4.16 = 82.16   | 83.08               | A 탈락  |
| 4      | 4       | 78 + 4.83 = 82.83   | 83.08               | A 탈락  |
| **5**  | **5**   | 78 + 5.37 = **83.37** | 83.08             | **A 진입** |

→ 5번 추천되면 3점 차이 역전. 기존도 추천되면 count 올라가지만, LLM이 안 찍으면 멈춤.

### 재평가에서의 동작
- 재평가 cull: **보너스 없이 score만으로 정렬**
- 보너스로 진입한 놈이 재평가에서 실력 증명 못하면 자연 도태
- 사냥에서 반복 추천 보너스로 진입 → 재평가에서 순수 경쟁 → 실력 없으면 탈락하는 순환 구조

## 변경 내역

### 1. `src/watchlist/db.rs` — `insert_candidate()`

```sql
-- 현재: pending일 때만 전부 갱신
ON CONFLICT(ticker) DO UPDATE SET
  name = excluded.name, ..., hunt_count = candidates.hunt_count + 1
  WHERE candidates.status = 'pending'

-- 변경: count는 항상 +1, 나머지는 pending일 때만
ON CONFLICT(ticker) DO UPDATE SET
  hunt_count = candidates.hunt_count + 1,
  name = CASE WHEN candidates.status = 'pending' THEN excluded.name ELSE candidates.name END,
  sector = CASE WHEN candidates.status = 'pending' THEN excluded.sector ELSE candidates.sector END,
  reason = CASE WHEN candidates.status = 'pending' THEN excluded.reason ELSE candidates.reason END,
  hunt_score = CASE WHEN candidates.status = 'pending' THEN excluded.hunt_score ELSE candidates.hunt_score END,
  market = CASE WHEN candidates.status = 'pending' THEN excluded.market ELSE candidates.market END,
  prompt_id = CASE WHEN candidates.status = 'pending' THEN excluded.prompt_id ELSE candidates.prompt_id END
```

### 2. `src/watchlist/pipeline.rs` — `run_cycle()` 감정 점수 계산

```rust
// 현재: score에 보너스 포함
let hunt_bonus = (candidate.hunt_count as f64).ln_1p() * hunt_count_weight;
let final_score = hunt_s * hunt_weight + jr.score * (1.0 - hunt_weight) + hunt_bonus;

// 변경: score = 기본 점수만
let final_score = hunt_s * hunt_weight + jr.score * (1.0 - hunt_weight);
```

재평가 쪽은 이미 보너스 없으니 변경 불필요.

### 3. `src/watchlist/db.rs` — `cull_excess_judged()` 보너스 동적 적용

```rust
// 현재
pub fn cull_excess_judged(max_survivors: usize) -> Result<usize>
// SQL: ORDER BY score DESC

// 변경: with_bonus 파라미터 추가
pub fn cull_excess_judged(max_survivors: usize, with_bonus: bool) -> Result<usize>
// with_bonus=true:  ORDER BY (score + ln(1 + hunt_count) * ?) DESC
// with_bonus=false: ORDER BY score DESC
```

SQLite에서 `ln()` 없으므로 Rust 쪽에서 전체 judged 로드 → 정렬 → 상위 N개 keep → 나머지 BL.
또는 DB에서 id, score, hunt_count를 읽어서 Rust에서 effective score 계산 후 정렬.

### 4. `src/watchlist/pipeline.rs` — cull 호출부

```rust
// run_cycle (사냥): 보너스 적용
db::cull_excess_judged(max_survivors, true)

// run_reeval (재평가): 보너스 없음 — 순수 실력
db::cull_excess_judged(max_survivors, false)
```

### 5. count 누적 경로 정리 (BL 필터 주의)

사냥 시 BL 필터링이 `insert_candidate` 호출 전에 걸리므로, blacklisted 상태에서는 count가 안 올라감.

| 상태 | 재추천 시 count | 비고 |
|------|----------------|------|
| pending | +1 | 현재와 동일 |
| collected | +1 | 새로 추가 |
| judged | +1 | 새로 추가 |
| blacklisted | 안 올라감 | BL 필터에 걸려서 insert 안 됨 |

패자 부활(revive) 시 count는 리셋 안 됨 (기존값 유지).

### 6. 테스트 수정

- `candidate_upsert_judged_skipped`: judged 재삽입 시 count 올라가는 것으로 변경 (1→2), 나머지 필드 보호 유지
- `cull_excess_judged` 테스트: with_bonus true/false 각각 검증
- 신규 추가: blacklisted 상태에서도 count 올라가는지 검증

### 6. 문서 업데이트

- CLAUDE.md: 점수 계산 설명, cull 동작 변경 반영
- docs/architecture.md: 동일

## 변경 파일 요약
- `src/watchlist/db.rs` — insert_candidate SQL, cull_excess_judged 시그니처+로직, 테스트
- `src/watchlist/pipeline.rs` — run_cycle 점수 계산, cull 호출부
- `CLAUDE.md` — 점수/도태 설명
- `docs/architecture.md` — 동일
