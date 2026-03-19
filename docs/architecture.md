# 구현 아키텍처

## main.rs 초기화 흐름

```
0. 수동 런타임 생성 (new_current_thread) + LocalSet::block_on()
1. Config::load() — 파일 없으면 템플릿 생성 후 종료
2. log 섹션 마이그레이션 — 누락 시 defaults 포함 자동 저장
3. 필수 설정 검증 (bot_token, app_key, app_secret, hts_id)
4. 로깅 초기화 (stdout: RUST_LOG, 파일: WARN 이상)
5. storage::init_config(config) — Config 인메모리 싱글턴 적재
6. API Actor 채널 생성 (mpsc, buffer=32) + spawn_local
7. 스케줄러 spawn_local (api_handle + tg_bot)
8. 텔레그램 봇 실행 (메인 태스크, block)
```

---

## Config 인메모리 싱글턴

`thread_local! RefCell<Option<Config>>`로 관리. current_thread 런타임이므로 단일 스레드 보장.

```rust
pub fn init_config(config: Config) { ... }      // 시작 시 1회
pub fn with_config<F, R>(f: F) -> R { ... }     // 읽기 전용 (클로저로 값 복사)
pub fn update_config<F>(f: F) -> Result<()> { ... } // 변경 + config.json 저장
```

---

## 봇 명령 처리 흐름

```
사용자 텔레그램 메시지
  → teloxide 디스패처 (비커맨드 → default_handler로 무시)
  → handle_command()
       ├─ [시세 불필요: /port add, /port remove, /signal*]
       │    SQLite SELECT → 변경 → SQLite INSERT → 응답
       │
       └─ [현재가 필요: /port list, /port info, /port summary]
            SQLite SELECT
            ├─ CART: cached_price 직접 사용
            └─ 기타: ApiHandle → API Actor (mpsc/oneshot) → 한투 API
            formatter → 응답 + 캐시 갱신
```

---

## Actor 간 통신

### 채널 구성
- **mpsc** (buffer=32): Bot Task → API Actor 단방향
- **oneshot**: 각 요청마다 생성. API Actor → 요청자 1회성 응답

```rust
pub enum ApiRequest {
    GetDomesticPrice  { symbol, respond_to },
    GetOverseasPrice  { exchange, symbol, respond_to },
    GetBondPrice      { isin, respond_to },
    GetExchangeRate   { respond_to },
    GetStockName      { prdt_type_cd, pdno, respond_to },
    GetOverseasDetail { exchange, symbol, respond_to },
}
```

### ApiHandle
`mpsc::Sender` 래퍼. Clone 가능 → 여러 태스크에서 공유.
호출 측은 채널을 모름. `api.get_domestic_price("005930").await?` 만 호출.

### API Actor 내부
`ActorContext`가 reqwest::Client, 토큰, rate limiter 독점 소유.
매 요청 전 `token_needs_refresh()` 체크 → 만료 1시간 전 자동 갱신.
해외주식 조회 시 `t_rate` → `usd_krw` 메모리 갱신.

### current_thread에서의 동작
싱글스레드, `.await`에서 cooperative 전환. Mutex 없이 채널로 동시성 확보.

---

## 스케줄러

`tokio::time::interval` + `tokio::select!` 루프.

```rust
loop {
    tokio::select! {
        _ = signal_tick.tick()        => { /* 장중이면 시그널 체크 */ }
        _ = hunt_tick.tick()          => { /* 사냥 사이클 */ }
        _ = discovery_trigger.notified() => { /* /w run 즉시 실행 */ }
        _ = reeval_tick.tick()        => { /* KST 02:00이면 재평가 */ }
    }
}
```

### 장시간 판단
```rust
fn is_market_hours() -> bool {
    let hhmm = hour * 100 + min;  // KST
    let krx_open = hhmm >= 900 && hhmm <= 1530;
    let us_open = hhmm >= 2230 || hhmm <= 500;
    krx_open || us_open
}
```
CART/BOND: `Market::is_open_now()` = false → 시그널 엔진에서 개별 스킵.

---

## 통신 복원력 (3중 방어)

한투 API 서버 idle connection ~60초 종료 → stale connection 재사용 오류 대응.

| 단계 | 메커니즘 | 상세 |
|------|----------|------|
| 1차 | `pool_idle_timeout(55s)` | 서버보다 짧게 설정, 대부분 차단 |
| 2차 | `send_with_retry` | 연결 오류(`is_request`/`is_connect`) 1회 재시도. 타임아웃 제외 |
| 3차 | 스케줄러 주기적 재시도 | 실패 시 다음 5분 주기에 자동 재시도 |

텔레그램: teloxide 내장 long-polling + exponential backoff 자동 복구.

---

## 시그널 판정 로직

5분 주기 REST 폴링 (WebSocket 미사용).

```
스케줄러 → is_market_hours()? → check_all_signals()
  1. holdings + signals 로드
  2. 활성(active=true) 시그널 순회
  3. market.is_open_now()? → 장외 스킵
  4. API Actor에 현재가 요청
  5. cached_price 갱신
  6. 조건 평가 → 발동 시 텔레그램 전송 + active=false
```

| 조건 | 판정 |
|------|------|
| `price_above` | `current >= target` |
| `price_below` | `current <= target` |
| `profit_above` | `(current - avg) / avg * 100 >= %` |
| `profit_below` | `(current - avg) / avg * 100 <= %` |

1회성 발동. 텔레그램 전송 **성공 시에만** `active: false`. 실패 시 다음 주기 재시도.

---

## 평가금액 계산

공통: `price × quantity × value_factor`

| 마켓 | factor | 비고 |
|------|--------|------|
| KRX | 1.0 | |
| NAS/NYS/AMS | 1.0 | 총합 시 × usd_krw |
| BOND | 0.1 | 가격=액면1만원 기준, 수량=1천원 단위 |
| CART | 1.0 | |

---

## 워치리스트 (US 소형주 디스커버리)

Google AI Studio (Flash Lite + Gemma) + 한투 API 조합.
프롬프트 미설정 시 동작 불가 (hunt/judge 각각 필수).

### 사냥 사이클 (`pipeline::run_cycle`)

```
0. cleanup_old_data(retention_days)

1. 사냥 — gemini::hunt() [Flash Lite, 1콜]
   프롬프트 + BL 목록 → Flash Lite → JSON 파싱 → HuntResult[]
   BL 필터링 → candidates(pending) 삽입
   ※ 상태 불문 hunt_count +1 (judged/collected 필드는 보호)

2. 수집 — 한투 API [pending 전부, 순차]
   fetch_detail(ticker, market_hint) → NAS→NYS→AMS 순회
   성공: collected + detail_text 저장
   실패: BL행

3. 평가 — gemini::judge() [Gemma, 1콜, 이번 사이클 수집분만]
   collected의 detail_text 합쳐서 Gemma → JudgeResult[]
   최종 등급 = hunt_score × W + judge_score × (1-W)  ← DB 저장 (보너스 미포함)
   score < min_score → 척살 (BL행)
   score >= min_score → 양피 (judged)
   감정 미매칭 → BL행

4. 도태 — cull_excess_judged(max_survivors, hunt_count_weight)
   effective = score + ln(1+hunt_count) × weight  ← 동적 계산 (사냥 보너스)
   상위 N개만 유지, 나머지 BL행
```

### 재평가 사이클 (`pipeline::run_reeval`, KST 02:00 하루 1회)

**hunt_count 보너스 미적용** — 순수 기본 점수로만 경쟁.

```
0. revive_near_misses() — 패자 부활 (score >= min_score*0.9 && strike < 3)
1. reset_judged_for_reeval() — judged 전부 → pending
2. 재수집 (사냥 사이클 2단계와 동일)
3. 재평가 (배치 분할: candidate_count 단위, 60초 간격, 보너스 없음)
4. 도태 — cull_excess_judged(max_survivors, 0.0)  ← 보너스 없이 순수 score
```

### 점수 계산

```
최종 등급(DB) = 사냥 매력 × w + 가죽 품질 × (1-w)    (w = hunt_weight, 기본 0.5)

척살 판정: 최종 등급 < min_score → BL행 (보너스 무관)

도태(cull) 판정:
  사냥 사이클: effective = 최종 등급 + ln(1+hunt_count) × hunt_count_weight
  재평가:      effective = 최종 등급  (보너스 없음)
```

**hunt_count 보너스** (사냥 도태 전용, weight=3.0):
| count | 보너스 |
|-------|--------|
| 1 | +2.1 |
| 5 | +5.4 |
| 10 | +7.2 |

- `hunt_count`: 사냥 추천 시 **상태 불문** 항상 +1 (BL은 필터링으로 미도달)
- 패자 부활 시 count 유지 (리셋 안 됨)
- 사냥 도태에서 반복 추천 신규가 기존 양피를 밀어냄
- 재평가 도태에서는 순수 경쟁 → 보너스로 진입한 놈이 실력 없으면 탈락

### 후보 상태 전이

```
hunt 추천 → pending  (신규 insert, 기존은 count만 +1)

pending → collected    (수집 성공)
pending → blacklisted  (수집 실패)
collected → judged     (감정 통과)
collected → blacklisted (감정 미달/미매칭)
judged → blacklisted   (cull 도태 or 재감정 미달)
judged → pending       (재평가 리셋)
blacklisted → pending  (패자 부활: score >= min_score*0.9 && strike < 3)
```

### Gemini 한도 체크 (이중)

| API | 본체 (gemini.rs) | 사전필터 (scheduler.rs) |
|-----|-----------------|----------------------|
| hunt | `hunt()` 진입부 | 사냥 사이클 진입 시 |
| judge | `judge()` 진입부 | 사냥/재평가 사이클 진입 시 |

### 429 대응
- **PerMinute**: retryDelay + 5초 대기 → 같은 모델 1회 재시도
- **RPD 등**: 즉시 다음 모델 폴백
- 전 모델 실패 → 마지막 에러 반환

### 모델 폴백
config 배열 순회. 당일 성공 모델 우선, 다음날(태평양시간 자정) 리셋.
Gemini 호출은 API Actor 미경유 (별도 reqwest::Client).
