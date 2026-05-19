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

### API Actor 내부 (파이프라인)
`ActorContext`가 토큰, rate limiter 독점 소유. `reqwest::Client`는 Clone(커넥션 풀 공유).
`FuturesUnordered` + `tokio::select!`로 발신은 rate limit(50ms) 직렬, 응답은 비동기 수거.

```
발신: [req1][50ms][req2][50ms][req3]...  → rate limit 준수
응답:      [────resp1────]                → 완료 순서대로 수거
                [────resp2────]
```

API 함수는 `(client, base_url, headers)` 받아서 독립 실행. Actor 상태 빌림 없음.
해외주식 응답 시 `t_rate` → `usd_krw` completion 핸들러에서 갱신.

### /port list 캐시
`cached_price` + `cached_at` age ≤ 1분 + price > 0이면 API 미호출. 장중 반복 조회 시 즉시 응답.

**price=0 방지**: API 응답 파싱 시 price ≤ 0 또는 empty → Error 반환. cached_price는 price > 0만 저장/폴백.

---

## 스케줄러

`tokio::time::interval` + `tokio::select!` 루프.

```rust
loop {
    tokio::select! {
        _ = signal_tick.tick()        => { /* 장중이면 시그널 체크 */ }
        _ = hunt_tick.tick()          => { /* 사냥 사이클 */ }
        _ = discovery_trigger.notified() => { /* /w hunt 즉시 실행 */ }
        _ = eval_tick.tick()          => { /* KST 02:00/14:00 가죽 작업 */ }
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

### 두 개의 독립 사이클 + 2테이블

| 사이클 | 함수 | 주기 | 역할 | 소유 테이블 |
|--------|------|------|------|------------|
| 사냥 | `run_hunt` | 30분 | Gemini 추천 → pending 삽입 | `pending` CRUD |
| 가죽 작업 | `run_evaluate` | 하루 2회 (KST 02:00, 14:00) | 수집 + 감정 + 도태 | `candidates` CRUD, `pending` READ |

```
pending (사냥 버퍼)              candidates (감정 완료)
  ticker UNIQUE                    ticker UNIQUE
  hunt_score, hunt_count           score, verdict, detail_text
  strike_count                     status: judged | blacklisted
  market, name, sector, reason     strike_count
```

### 파이프라인과 상태 전이

```
사냥 사이클:
  cleanup(졸업한 pending 삭제) → Gemini 추천 → pending (hunt_count +1)

가죽 작업:
  ① 부활: candidates(BL) → pending  (score ≥ min×0.9, strike < 3)
  ② 수집: pending → KIS API → candidates 졸업 (count 합산)
     pending 실패 → strike+1 (pending 유지)  judged 실패 → 스킵
     strike ≥ 3 시에만 candidates(BL) 전환  (단발 장애로 영구 BL 금지)
  ③ 감정: score < min → BL                  score ≥ min → judged
  ④ 도태: 상위 N 외 → BL                    (effective score 기준)
```

### 점수와 보너스/감점

```
score(DB) = judge_score                            ← 순수 데이터 평가
effective  = score + ln(1 + hunt_count) × weight    ← 도태 판정용
```

| 요인 | 적용 시점 | 효과 |
|------|----------|------|
| **min_score (60)** | 감정 직후 | 절대 마지노선. 미달 즉시 BL. 보너스 무관 |
| **hunt_count 보너스** | 도태(cull) | 반복 추천 종목 보호. ln 포화 (count=100 → +13.8 한계) |
| **삼진아웃 (strike ≥ 3)** | 부활 판정 / 수집 실패 | 점수 척살은 부활 불가, 수집 실패는 BL 전환. retention(100일) 후 자연소멸 |
| **수집 실패** | 수집 | strike+1 → 3회 누적 시 BL (단발 장애 보호) |
| **수동 척살** | `/w bl add` | 영구 BL (score=NULL, strike_count 무관) |

### Gemini 호출 정책

- **요청 본문**:
  - `generationConfig.responseMimeType="application/json"` — JSON 형식만 출력
  - `generationConfig.responseSchema` — hunt/judge 각각 OpenAPI 3.0 subset 스키마 주입. 디코더가 schema 위반 토큰을 마스킹해 구조 깨짐(닫는 괄호 hallucinate, reason 안에 추가 따옴표 등) 원천 차단
  - 프롬프트에 markdown 펜스 / 인라인 schema 예시 금지. 구조는 schema가 강제하므로 의미(시장 코드 enum, 점수 범위)만 자연어로 명시
- **HTTP 타임아웃**: 60초 (`call_llm` 내부). hang 방지
- **429 PerMinute**: `retryDelay + 5초` 대기 → 같은 모델 1회 재시도
- **429 PerDay / 4xx / 5xx**: 즉시 다음 모델 폴백 (`call_llm_with_fallback`)
- **응답 파싱**: `extract_json_array` 실패 또는 `Vec<HuntResult>` / `Vec<JudgeResult>` 역직렬화 실패 시 `prompt_history.status='parse_error'`로 박고 bail (묵음 폴백 금지). 파싱 실패는 폴백 루프 밖이라 다음 모델로 안 넘어감 — schema 강제로 구조 깨짐을 사전 차단하는 게 1차 방어선
- config 배열 순회, 당일 성공 모델 우선. Gemini 호출은 API Actor 미경유.
