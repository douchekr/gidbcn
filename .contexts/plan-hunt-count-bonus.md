# feat: hunt_count 가중치 — 반복 추천 보너스

## 배경
max_survivors를 100으로 늘려도, 기존 양피가 재평가에서 같은 점수를 유지하면 신규가 못 들어옴.
여러 번 사냥에서 추천된 종목(hunt_count > 1)은 그만큼 "핫"하다는 시그널 → 보너스 부여.

## 현재 점수 계산
```rust
final_score = hunt_score × hunt_weight + judge_score × (1 - hunt_weight)
// hunt_weight = 0.5 (config)
```
hunt_count는 DB에 저장만 되고 점수에 반영 안 됨.

## 현재 hunt_count 분포 (judged 50마리)
| hunt_count | 수 |
|---|---|
| 1 | 41 |
| 2 | 2 |
| 3 | 3 |
| 4 | 1 |
| 9 | 1 |
| 10 | 1 |
| 19 | 1 |

## 방안: 로그 스케일 보너스

```rust
let hunt_bonus = (candidate.hunt_count as f64).ln_1p() * HUNT_COUNT_WEIGHT;
final_score = hunt_score × W + judge_score × (1-W) + hunt_bonus;
```

`ln_1p(x)` = ln(1+x):
| hunt_count | ln(1+count) | × 3.0 |
|---|---|---|
| 1 | 0.69 | 2.1 |
| 2 | 1.10 | 3.3 |
| 3 | 1.39 | 4.2 |
| 5 | 1.79 | 5.4 |
| 10 | 2.40 | 7.2 |
| 19 | 3.00 | 9.0 |

- 1회 추천 = +2점, 반복 추천 = 최대 +9점 정도
- 로그 스케일이라 hunt_count 폭주해도 점수가 무한히 올라가지 않음
- HUNT_COUNT_WEIGHT(기본 3.0)는 config에 추가

## 적용 위치
`src/watchlist/pipeline.rs` — run_cycle, run_reeval 동일 패턴:
```rust
// 현재
let final_score = hunt_s * hunt_weight + jr.score * (1.0 - hunt_weight);

// 변경
let hunt_bonus = (candidate.hunt_count as f64).ln_1p() * hunt_count_weight;
let final_score = hunt_s * hunt_weight + jr.score * (1.0 - hunt_weight) + hunt_bonus;
```

## config 추가
`watchlist.hunt_count_weight` (기본 3.0)
- 0.0이면 비활성 (기존 동작)
- 높을수록 반복 추천 보너스 큼

## 변경 파일
- `src/config.rs` — `hunt_count_weight` 필드 추가 (기본 3.0)
- `src/watchlist/pipeline.rs` — run_cycle, run_reeval 점수 계산
- CLAUDE.md — watchlist 설정 테이블에 추가
