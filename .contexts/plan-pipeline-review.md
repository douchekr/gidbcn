# 사냥 파이프라인 개선 계획

## 대원칙

> 사냥 프롬프트로 개잡주를 발굴한다 (인상 비평).
> 추천된 놈은 한투 데이터 기반으로 평가해서 점수 미만은 탈락.
> 반복하면 개잡주지만 쓸만한 백그라운드를 가진 놈들이 추려진다.

---

## 현재 구조와 문제

### 현재: 3개 사이클, 사냥이 감정을 잡고 있음

```
사냥 사이클 (30분)     : hunt → collect → judge(이번 것만) → cull
재평가 사이클 (02:00)  : revive → reset judged→pending → collect → judge(배치) → cull
```

**문제**: 사냥 실패/빈 결과 → 감정 전부 스킵 → collected 113개 방치, 양피 진입 0.

---

## 변경안: 두 개의 독립 사이클

### 독립 사이클의 의미

현재는 사냥과 감정이 `run_cycle` 한 함수에 직렬 묶임.
사냥이 실패하면 감정도 죽고, 감정이 느려도 사냥을 못 함.

분리하면 **각 사이클이 상대의 성패와 무관하게 자기 일만 함**:
- 사냥을 멈춰도(현재 휴지 상태) 감정은 계속 돌아감
- 감정이 오래 걸려도 사냥 스케줄에 영향 없음
- collected 적체 = 다음 감정 사이클에서 자동 해소

### 사냥 사이클 (hunt)

```
hunt → pending
 Gemini   DB insert
```

- **역할**: 후보 발굴만. Gemini 1콜 + DB insert. **수집(KIS API) 안 함.**
- **주기**: 설정 가능 (현재 30분). 휴지/장주기 가능.
- **실패 영향**: 자기만 실패. pending/collected 풀에 영향 없음.

### 감정 사이클 (evaluate = 현재 reeval 확장)

```
revive → collect(pending+judged) → judge(배치) → cull
              한투 API                Gemini
```

- **역할**: 수집 + 감정 + 도태. 모든 무거운 작업을 한 곳에서.
- **주기**: 하루 2회 (예: KST 02:00, 14:00)
- **대상**: pending(사냥 신규 + 부활) + judged(기존 양피)

**collected 상태 제거**: 수집→감정이 같은 함수에서 연속 실행.
"데이터 있고 감정 대기 중"이라는 상태가 함수 밖에 노출될 필요 없음.

```
기존 상태: pending → collected → judged → BL  (4개)
변경:      pending → judged → BL              (3개)
```

**수집 실패 분기**:
| 대상 | 실패 시 | 이유 |
|------|---------|------|
| pending | BL (영구) | 한투 조회 불가 = 종목 자체 문제 |
| judged | 스킵 (기존 score 유지) | 일시적 API 오류일 수 있음, 다음 사이클에 재시도 |

### 배치 전략

**DB 상태가 아닌 함수 내 메모리가 배치 대상을 결정.**

```
1. 수집: pending+judged → KIS API → 성공분을 ready Vec에 축적 + detail_text DB 저장
2. 감정: ready.chunks(candidate_count) 배치 분할, 배치 간 60초 대기
3. cull
```

collected 상태 없어도 배치 동작에 문제 없음.
현재도 `collected` 조회 → `chunks()` → 감정인데, `ready` Vec → `chunks()` → 감정으로 대체.

**배치 중간 실패 시 복원**:
| 상황 | pending이었던 놈 | judged였던 놈 |
|------|-----------------|--------------|
| 수집 성공 + 감정 성공 | → judged | → judged (score 갱신) |
| 수집 성공 + 배치 실패 | pending 유지 | judged 유지 (old score) |
| 수집 실패 | → BL (영구) | 스킵 (judged 유지) |

배치 실패 시: 다음 감정 사이클에서 재수집+재감정. detail_text 덮어쓰므로 정합성 문제 없음.

삭제 대상: `reset_judged_for_reeval()`, `update_candidate_collected()`, `CandidateStatus::Collected`,
`count_candidates_by_status(Collected)`, `list_candidates(Collected)` 조회 경로.

### RPD/TPM 영향 분석

감정 사이클 1회: ~100후보 ÷ 30(batch) = **4 judge콜**, 배치 간 60초, 소요 ~3분.

| API | 하루 콜 | 한도 | 비고 |
|-----|---------|------|------|
| hunt | ~20 | 20/일 | 30분 주기, 기존과 동일 |
| judge | 4×2 = 8 | 14,400/일 | 여유 충분 |

TPM: ~5,750토큰/배치, 60초 간격 → 무료 한도 내.
429 대응: 기존 전략(PerMinute 재시도, PerDay 모델 폴백) 변경 없음.

### 왜 수집을 사냥에서 뺐는가

현재 구조에서 collect(한투 API)가 사냥 사이클과 감정 사이클 양쪽에 중복.

| 작업 | 사냥 사이클 (현재) | 감정 사이클 | 중복? |
|------|-------------------|------------|-------|
| hunt → pending | ✅ | ❌ | 사냥만 |
| collect pending → collected | ✅ | ✅ | **중복** |
| judge collected → judged/BL | ❌ | ✅ | 감정만 |
| cull | ❌ | ✅ | 감정만 |

수집을 감정 사이클로 일원화하면:
- 사냥 사이클 = Gemini 콜 1번. 단순.
- KIS API 호출이 감정 사이클 한 곳에서만 발생 → 실패 처리도 한 곳
- 사냥에서 수집 실패 → BL행 경로 제거 (감정 사이클에서 일괄 처리)

### 상태 전이 (3상태)

상태: `pending`, `judged`, `blacklisted`

```
사냥 사이클           감정 사이클 (하루 2회)
───────────          ─────────────────────
hunt 추천 → pending    revive: BL(척살) → pending

                       collect(pending+judged):
                         pending 실패 → BL (영구)
                         judged 실패  → 스킵 (score 유지)

                       judge(수집 성공분):
                         통과 → judged
                         미달/미매칭 → BL (strike+1)

                       cull:
                         도태 → BL (strike+1)
```

사냥: `→ pending` 1개.
감정: `pending→judged`, `pending→BL`, `judged→BL`, `BL→pending` 4개.
합계 **5개 전이** (기존 8개에서 축소).

### 보고 메시지

```
사냥 사이클:
  🎯 사냥 보고 (🔍포착 +N)

감정 사이클:
  🔄 감정 보고 (대상 N마리 🔁해제 +R → 📦수집 +C ❗F → 🦎양피 +A 🗡️척살 +B)
```

사냥 보고는 포착 수만. 수집/감정 결과는 감정 사이클 보고에 통합.

---

## 장치별 유효성

### hunt_count 보너스 — ✅ 유지, 감정 사이클에도 적용

운영 데이터:
- 75~79점 52마리, 70~74점 39마리 — 동점 과밀 구간이 존재
- 신규 76점이 들어와도 기존 76점이 꽉 차 있으면 cull에서 밀림
- 보너스의 역할 = **min_score 돌파가 아니라, 동점 과밀 구간에서 자주 추천된 놈에게 틈을 만들어주는 것**

효과 예시 (weight=3.0):
| count | 보너스 | 76점 기준 effective |
|-------|--------|-------------------|
| 1 | +2.1 | 78.1 |
| 3 | +4.2 | 80.2 |
| 5 | +5.4 | 81.4 |

LLM이 5번 추천한 76점짜리 → effective 81.4 → 기존 80점대에 끼어들 수 있음.

**변경**: 감정 사이클(=재평가 통합)에서도 보너스 적용.
현재 재평가는 `cull_excess_judged(max, 0.0)`인데, 이를 `cull_excess_judged(max, hunt_count_weight)`로.
"재평가는 순수 실력" 원칙 폐기 → **모든 도태에 보너스 통일**.

### hunt_count 보너스 use case

보너스는 **cull(도태)에서만** 적용. min_score 컷은 보너스 무관 = 절대 마지노선.

```
① 감정: score = judge_score
② 척살: score < min_score(60) → BL행 ← 보너스 무관
③ 도태: effective = score + ln(1+count) × weight → 상위 N만 생존
```

| 종목 | judge | count | bonus | effective | min_score | cull |
|------|-------|-------|-------|-----------|-----------|------|
| A | 55 | 20 | +9.1 | — | ❌ 척살 | 도태까지 안 감 |
| B | 62 | 20 | +9.1 | 71.1 | ✅ | 71점대와 경쟁 |
| C | 68 | 1 | +2.1 | 70.1 | ✅ | B(71.1)에게 밀림 |

**"데이터 점수 낮지만 자주 추천돼서 살아남는" 시나리오**: 의도된 동작.

- 대원칙이 "인상 비평으로 개잡주 발굴" → LLM이 20번 추천 = 그만큼 인상적
- min_score(60)은 절대 마지노선, 보너스로 뚫을 수 없음
- 보너스는 로그 포화: count=100이어도 `ln(101)×3 = 13.8`이 한계
- **최소 기준(min_score) + 지속적 LLM 관심 = 살아남을 자격 있음**

### 삼진아웃 (strike ≥ 3) — ✅ 유효

- 척살 3회 누적 → revive 대상 제외 → retention(100일) 후 자연소멸
- 변경 불필요

### 수집 실패 BL (영구) — ✅ 유효

- score=NULL → revive 조건 미충족 → 사실상 영구
- 한투 3대 거래소 미존재 = 재시도 무의미
- 변경 불필요

### 도태 (cull) — ✅ 유효

- max_survivors 초과분 하위 척살
- 감정 사이클 통합 시 항상 `hunt_count_weight=0` (순수 실력)
- 변경 불필요

### 패자 부활 — ✅ 유효

- 감정 사이클 시작 시 실행 (기존 재평가와 동일)
- 변경 불필요

### hunt_weight — ⚠️ 폐기 권장 (W=0)

**역할 중복 문제**: hunt_weight와 hunt_count 보너스가 둘 다 "LLM이 좋아하는 종목 우대".

| 장치 | 입력 | 영향 | 성격 |
|------|------|------|------|
| hunt_weight | hunt_score (1회 인상 점수) | base score에 혼입 | 정적 (최초 1회 고정) |
| hunt_count 보너스 | count (추천 횟수 누적) | cull effective score | 동적 (매 사냥마다 성장) |

- hunt_score는 LLM이 데이터 없이 매긴 "감" 점수, 최초 pending 때 1회 고정
- 감정 사이클마다 judge_score는 갱신되지만 hunt_score는 그대로 → 시간이 지나면 노이즈
- hunt_count 보너스가 "LLM 선호도"를 더 정확하게 표현 (횟수 = 지속적 관심)

**결론**: score = 순수 데이터 평가(judge_score만), LLM 선호도는 hunt_count 보너스로 표현.
`hunt_weight=0` (config 변경) 또는 코드에서 hunt_score 혼입 로직 제거.

### hunt_count 라이프사이클

```
[탄생] 사냥 추천 → insert_candidate
  └─ 신규: hunt_count = 1 (DEFAULT)
  └─ 기존(상태 불문): hunt_count += 1 (UPSERT)

[유지] 상태 전이에서 리셋 안 됨
  └─ pending → collected → judged : 유지
  └─ judged → BL (척살/도태)       : 유지
  └─ BL → pending (패자 부활)      : 유지 (score/verdict만 NULL)
  └─ judged → pending (재평가 리셋) : 유지

[성장] 사냥 사이클마다
  └─ 이미 judged/collected/pending인 종목이 다시 추천되면 +1
  └─ BL 종목은 사냥 필터링에서 걸려서 미도달 → 증가 안 함

[소멸] cleanup_old_data(retention_days)
  └─ candidate 삭제 → count도 함께 삭제
  └─ 이후 같은 ticker 재추천 시 count=1로 재시작
```

**특성**: BL 동안은 동결 (사냥 필터에 걸림), 부활 시 기존 count 가지고 복귀.

---

## 아키텍처 문서 정합성

| 위치 | 현재 | 변경 후 |
|------|------|---------|
| architecture.md:171 | `BL 목록 → Flash Lite` | ✅ `BL 사후 필터링`으로 수정 완료 |
| architecture.md:165~190 | 사냥 사이클 (hunt+collect+judge 직렬) | 사냥/감정 분리, 감정=재평가 통합 |
| architecture.md:192~202 | 재평가 사이클 (하루 1회) | 하루 2회, 사냥 사이클의 judge 역할 흡수 |

---

## 구현 순서

| 단계 | 작업 | 파일 |
|------|------|------|
| 1 | `CandidateStatus::Collected` 제거, 3상태로 단순화 | models.rs, db.rs |
| 2 | `run_cycle` → `run_hunt` (Gemini + DB insert만) | pipeline.rs |
| 3 | `run_reeval` → `run_evaluate` (collect pending+judged → judge → cull) | pipeline.rs |
| 4 | hunt_weight 혼입 제거 (score = judge_score만) | pipeline.rs |
| 5 | cull 보너스 통일 | pipeline.rs |
| 6 | 불필요 함수 삭제 (`reset_judged_for_reeval`, `update_candidate_collected` 등) | db.rs |
| 7 | 스케줄러: 사냥/감정 독립 타이머 + 하루 2회 + 보고 분리 | scheduler.rs |
| 8 | 아키텍처 문서 동기화 | architecture.md |
| 9 | 빌드 + 테스트 | - |

Step 1~8 한 커밋.
