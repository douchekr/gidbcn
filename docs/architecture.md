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

### 두 개의 독립 사이클

| 사이클 | 함수 | 주기 | 역할 |
|--------|------|------|------|
| 사냥 | `run_hunt` | 30분 (설정 가능) | Gemini 추천 → pending 삽입 |
| 감정 | `run_evaluate` | 하루 2회 (KST 02:00, 14:00) | 수집 + 감정 + 도태 |

사냥 실패해도 감정은 독립 실행. 감정이 오래 걸려도 사냥에 영향 없음.

### 파이프라인과 상태 전이

상태 3개: `pending`, `judged`, `blacklisted`

```
사냥 사이클:
  Gemini 추천 → pending (hunt_count +1)

감정 사이클:
  ① 부활: BL → pending            (score ≥ min×0.9, strike < 3)
  ② 수집: pending/judged → 한투 API
     pending 실패 → BL (영구)      judged 실패 → 스킵
  ③ 감정: score < min → BL        score ≥ min → judged
  ④ 도태: 상위 N 외 → BL          (effective score 기준)
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
| **삼진아웃 (strike ≥ 3)** | 부활 판정 | 3회 척살 → 부활 불가. retention(100일) 후 자연소멸 |
| **수집 실패** | 수집 | pending → 영구 BL (score=NULL → 부활 불가) |

### Gemini 429 대응 + 모델 폴백

- **PerMinute 429**: retryDelay + 5초 대기 → 같은 모델 1회 재시도
- **PerDay 등**: 즉시 다음 모델 폴백
- config 배열 순회, 당일 성공 모델 우선. Gemini 호출은 API Actor 미경유.
