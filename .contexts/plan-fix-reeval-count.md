# fix: 재평가/사이클 보고 숫자 불일치

## 문제
`🔄 가죽 점검 (282마리 🔁해제 +223 → 🦎양피 +206 🗡️척살 +216)`
양피(206) + 척살(216) = 422 ≠ 타겟(282). 로직은 정상, 보고 카운트만 틀림.

## 실제 로그 분석 (3/18)
```
패자 부활: 223개
대상 282개, 수집 273개, 생존 206개, 척살 216개, 실패 0개
척살: 156개 (상위 50개 유지)
배치 감정 합계: 266개
```
- target 282 = reset(50) + revived(223) + stale_collected(9)
- 수집 273 = pending만 처리 (stale 9개는 이미 collected라 건너뜀)
- 감정 266 = Gemma 반환 매칭 (282 - 266 = 16개 티커 미반환)
- survived 206 = 감정 통과 (cull 전), culled 216 = 미달(60) + cull(156)

## 버그 3개 (모두 보고 카운트, 로직은 정상)

### 1. 양피 부풀림 — `survived -= cull_excess` 누락
`cull_excess_judged` 후 survived를 안 줄임. 실제 양피 50인데 206으로 표시.

**수정** (`pipeline.rs`, run_cycle + run_reeval 동일):
```rust
let culled_excess = db::cull_excess_judged(max_survivors).unwrap_or(0);
report.culled += culled_excess;
report.survived = report.survived.saturating_sub(culled_excess);  // 추가
```

### 2. stale_collected 카운트 누락
stale_collected가 target에 포함되지만 collected 카운트에 안 잡힘 → 수집 갭 미설명.

**수정** (`pipeline.rs`, run_reeval):
```rust
report.collected += stale_collected;  // L219 뒤에 추가
```
stale는 이미 수집 완료된 거니까 collected에 포함시키면 됨.

### 3. 감정 미매칭 — Gemma가 티커 안 돌려준 경우
judge에서 반환 안 된 collected는 survived에도 culled에도 안 잡힘. collected 상태 잔류.
이건 다음 재평가에서 stale_collected로 자연 처리됨. 카운트만 안 맞는 것.

**수정**: 감정 루프에서 매칭된 id 추적 → 루프 후 미매칭 collected를 BL행
```rust
// 감정 루프에서 matched_ids 수집
// 루프 후:
for c in &collected {
    if !matched_ids.contains(&c.id) {
        let _ = db::add_blacklist(&c.ticker, "감정 누락 (자동)");
        let _ = db::update_candidate_status(c.id, CandidateStatus::Blacklisted);
        report.culled += 1;
    }
}
```
run_cycle, run_reeval 둘 다 적용.

## 변경 파일
- `src/watchlist/pipeline.rs` — run_cycle, run_reeval

## 검증
- `cargo test pipeline` — 기존 테스트 수정 (숫자 변경)
- `cargo test` — 전체 통과
- 배포 후 다음 재평가 로그로 숫자 정합성 확인
