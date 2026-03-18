# 구현 아키텍처

## 봇 명령 처리 흐름

텔레그램 명령 수신부터 파일 저장까지의 흐름:

```
사용자 텔레그램 메시지
  → teloxide 디스패처 (handler.rs)
      비커맨드 메시지 → default_handler로 무시
  → handle_command() (commands.rs)
       │
       ├─ [시세 조회 없는 명령: /port add, /port remove, /port edit, /signal*]
       │    storage::load_*()          // SQLite SELECT (portfolio.db)
       │    데이터 변경
       │    storage::save_*()          // SQLite DELETE+INSERT (트랜잭션)
       │    응답 문자열 반환
       │
       └─ [현재가 필요한 명령: /port list, /port info, /port summary]
            storage::load_portfolio()  // SQLite SELECT
            ┌─ CART 종목: cached_price 직접 사용 (API 호출 스킵)
            └─ 기타 종목: api.get_price_for_market() ──► API Actor (mpsc)
                                                          reqwest → 한투 API
                                                      ◄── PriceData (oneshot)
            formatter로 메시지 생성
            [캐시 갱신] storage::save_portfolio()  // SQLite 저장
            응답 문자열 반환
  → bot.send_message()
```

**데이터 저장**: `/opt/kkuepark/gidbcn/portfolio.db` (SQLite, WAL 모드)
- `holdings` 테이블 — 전체 사용자 포트폴리오 (user_id 컬럼)
- `signals` 테이블 — 전체 사용자 시그널 (user_id 컬럼)
- `candidates`, `blacklist`, `prompts` 등 워치리스트 테이블도 동일 DB

**동기 I/O**: rusqlite 동기 호출 (current_thread에서 블로킹 무시 가능, `thread_local! RefCell<Connection>`)

---

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
// storage.rs
thread_local! {
    static IN_MEMORY_CONFIG: RefCell<Option<Config>> = RefCell::new(None);
}

pub fn init_config(config: Config) { ... }      // 시작 시 1회
pub fn with_config<F, R>(f: F) -> R { ... }     // 읽기 전용
pub fn update_config<F>(f: F) -> Result<()> { ... } // 변경 + 파일 저장
```

- `with_config`: 읽기 전용 접근. 클로저로 필요한 값만 복사해서 반환.
- `update_config`: 메모리 변경 + `config.json` 파일 저장. 토큰 갱신, 오너 등록 등에 사용.

---

## 스케줄러

`tokio::time::interval` 기반 `tokio::select!` 루프. `tokio-cron-scheduler` 미사용.

```rust
pub async fn run_scheduler(api, bot, discovery_enabled, discovery_trigger) {
    let mut signal_tick = interval(signal_interval);   // 기본 5분
    let mut hunt_tick = interval(hunt_interval);       // 기본 30분
    let mut reeval_tick = interval(Duration::from_secs(60));  // 1분마다 시각 체크

    loop {
        tokio::select! {
            _ = signal_tick.tick() => { /* 장중이면 시그널 체크 */ }
            _ = hunt_tick.tick() => { /* 사냥 사이클 */ }
            _ = discovery_trigger.notified() => { /* /w run 즉시 실행 */ }
            _ = reeval_tick.tick() => { /* KST 02:00이면 재평가 */ }
        }
    }
}
```

| 작업 | 주기 | 진입 조건 |
|------|------|-----------|
| 시그널 체크 | 5분 | `is_market_hours()` |
| 사냥 사이클 | 30분 | `discovery_enabled && prompts_configured && !hunt_exhausted && !judge_exhausted` |
| 즉시 실행 | 트리거 | `!hunt_exhausted && !judge_exhausted` |
| 재평가 | 1분 체크 | `discovery_enabled && prompts_configured && KST 02시 && 오늘 미실행 && !judge_exhausted` |

### 장시간 판단

```rust
fn is_market_hours() -> bool {
    let hhmm = hour * 100 + min;  // KST 기준
    let krx_open = hhmm >= 900 && hhmm <= 1530;
    let us_open = hhmm >= 2230 || hhmm <= 500;
    krx_open || us_open
}
```

- KRX와 US 장 시간대 중 하나라도 열려있으면 시그널 체크 실행
- CART/BOND 종목: `Market::is_open_now()` = false → 시그널 엔진에서 개별 스킵
- **환율 별도 스케줄 없음**: 해외주식 시세 조회 시 `t_rate` 부산물로 actor 메모리 갱신

### 토큰 갱신 전략

두 시스템의 토큰 방식이 근본적으로 다름:

| | KIS Open API | Telegram Bot API |
|---|---|---|
| 토큰 유형 | OAuth (client_credentials) | 고정 API 키 (BotFather 발급) |
| 유효기간 | 24시간 | 무기한 (수동 revoke 전까지) |
| 갱신 필요 | O — 자동 갱신 | X — 갱신 불필요 |
| 발급 제한 | 1분당 1회 | 없음 |

**KIS 토큰 자동 갱신 흐름**:
1. API Actor가 매 요청 전 `token_needs_refresh()` 호출
2. 만료 1시간 전이면 `issue_token()` → POST `/oauth2/tokenP`
3. 새 토큰을 `update_config()`로 config.json에 저장
4. 결과적으로 약 23시간 간격으로 갱신 (증권사에서 토큰 발급 확인 문자 발송됨)

**텔레그램 봇 토큰**: config.json에 한 번 설정하면 끝. 런타임 갱신 로직 없음.

---

## Actor 간 통신

### 채널 구성

```
main.rs에서 생성:
  let (api_tx, api_rx) = mpsc::channel::<ApiRequest>(32);
```

- **mpsc (Multi-Producer, Single-Consumer)**: Bot Task → API Actor 단방향. 버퍼 32.
- **oneshot**: 각 요청마다 생성. API Actor → 요청자 방향 1회성 응답.

### 통신 흐름

```
요청-응답 (시세 조회):

  Bot Task / 스케줄러 / 시그널 엔진
       │
       ├─ oneshot::channel() 생성
       ├─ mpsc.send(ApiRequest { ..., respond_to: tx }) ──►  API Actor
       │                                                       ├─ rate_limit() (50ms)
       │                                                       ├─ send_with_retry() → 한투 API
       ◄── rx.await (응답 대기)  ◄── respond_to.send(Result) ──┘
```

토큰 갱신은 actor 내부에서 자체 처리 (외부 메시지 없음).

### ApiHandle

`mpsc::Sender`를 감싸는 핸들. Clone 가능 → 여러 태스크에서 공유.

```rust
#[derive(Clone)]
pub struct ApiHandle {
    sender: mpsc::Sender<ApiRequest>,
}

impl ApiHandle {
    pub async fn get_domestic_price(&self, symbol: &str) -> Result<PriceData> {
        let (tx, rx) = oneshot::channel();
        self.sender.send(ApiRequest::GetDomesticPrice {
            symbol: symbol.to_string(),
            respond_to: tx,
        }).await?;
        rx.await?
    }

    /// Market에 따라 적절한 현재가 API 호출
    pub async fn get_price_for_market(&self, market: Market, symbol: &str) -> Result<PriceData> {
        match market {
            Market::KRX  => self.get_domestic_price(symbol).await,
            Market::NAS | Market::NYS | Market::AMS
                         => self.get_overseas_price(market.exchange_code(), symbol).await,
            Market::BOND => { /* bond → PriceData 변환 */ },
            Market::CART => anyhow::bail!("CART: 수동 관리 종목"),
        }
    }
}
```

호출하는 쪽은 내부 채널을 모름. 그냥 `api.get_domestic_price("005930").await?` 하면 됨.

### 메시지 타입

```rust
pub enum ApiRequest {
    GetDomesticPrice { symbol, respond_to: oneshot::Sender<Result<PriceData>> },
    GetOverseasPrice { exchange, symbol, respond_to: ... },
    GetBondPrice     { isin, respond_to: ... },
    GetExchangeRate  { respond_to: ... },
    GetStockName     { prdt_type_cd, pdno, respond_to: ... },
}
```

### API Actor 내부

`ActorContext`가 reqwest::Client, 토큰, rate limiter를 독점 소유. 외부 접근 불가.

```rust
pub async fn run_api_actor(mut rx: mpsc::Receiver<ApiRequest>) {
    let kis_config = storage::with_config(|c| c.kis_api.clone());
    let mut ctx = ActorContext::new(kis_config);
    let mut usd_krw: f64 = 1350.0;  // 해외주식 조회 시 t_rate로 갱신

    while let Some(req) = rx.recv().await {
        if auth::token_needs_refresh(&ctx.config.token) {
            ctx.refresh_token().await;  // 내부에서 update_config() 저장
        }
        match req {
            ApiRequest::GetDomesticPrice { symbol, respond_to } => {
                ctx.rate_limit().await;
                let result = domestic::get_price(&ctx, &symbol).await;
                let _ = respond_to.send(result);
            }
            ApiRequest::GetOverseasPrice { exchange, symbol, respond_to } => {
                ctx.rate_limit().await;
                let result = overseas::get_price(&ctx, &exchange, &symbol).await;
                if let Ok((_, Some(rate))) = &result {
                    usd_krw = *rate;  // t_rate 부산물로 환율 갱신
                }
                let _ = respond_to.send(result.map(|(price, _)| price));
            }
            // ... 나머지 동일 패턴
        }
    }
}
```

### 통신 복원력 (3중 방어)

실 운영 중 발생한 "connection closed before message completed" 오류 대응으로 단계적 방어 구축.

**경위**: 한투 API 서버가 idle connection을 ~60초에 끊는데, reqwest의 keep-alive 풀이 stale connection을 재사용하며 오류 발생. 두 차례 보강으로 해결.

```
┌─────────────────────────────────────────────────────────────┐
│ 1차 방어: pool_idle_timeout(55s)                             │
│   서버 idle timeout(~60s)보다 짧게 설정하여 stale connection  │
│   사전 제거. 대부분의 오류가 여기서 차단됨.                     │
│                                                              │
│ 2차 방어: send_with_retry (ActorContext)                      │
│   그래도 빠진 stale connection → 1회 재시도.                   │
│   - is_request / is_connect 오류만 재시도                     │
│   - is_timeout은 재시도 안 함 (대기 시간 2배 방지)             │
│   - try_clone()으로 요청 복제 → 실패 시 원본 에러 반환         │
│                                                              │
│ 3차 방어: 시그널 엔진 주기적 재시도                             │
│   개별 종목 시세 조회 실패 → 에러 로그 + 스킵.                 │
│   다음 스케줄러 주기(5분)에 자동 재시도.                       │
│   텔레그램 전송 실패 → active 유지 → 다음 주기에 재전송.       │
└─────────────────────────────────────────────────────────────┘
```

**텔레그램 봇 복원력** (teloxide 내장):
- long-polling (`getUpdates`) 기반 — 서버 push 아닌 클라이언트 pull
- 네트워크 단절 → teloxide 내부에서 exponential backoff 재시도
- 복구 시 `getUpdates`의 offset 기반으로 누락 없이 이어감
- `enable_ctrlc_handler()` 외에는 종료 조건 없음 → Ctrl+C 전까지 무한 복구

**에러 로깅 개선**:
- 모든 에러 출력: `{e}` → `{e:#}` (anyhow 에러 체인 전체 출력)
- HTTP 응답 body 포함 로깅 → 원인 추적 용이
- API Actor 내부에서 토큰 갱신 실패도 로그 (서비스 중단 없이 다음 요청에서 재시도)

**reqwest 클라이언트 설정** (ActorContext):
| 설정 | 값 | 이유 |
|------|-----|------|
| `connect_timeout` | 5초 | TCP+TLS 연결 수립 |
| `timeout` | 15초 | 전체 요청 (연결~응답 수신) |
| `pool_idle_timeout` | 55초 | stale connection 1차 방어 |
| TLS | rustls | OpenSSL 무의존 |

### current_thread에서의 동작

싱글스레드라 실제 동시 실행은 없음. `.await` 지점에서 제어권이 이벤트 루프로 반환되어 cooperative하게 번갈아 실행:

1. 봇 핸들러가 `api.get_price().await` 호출
2. mpsc에 메시지 넣고 oneshot rx를 `.await` → **제어권 반환**
3. API Actor의 `rx.recv().await`가 깨어남
4. reqwest HTTP 요청 `.await` → **제어권 반환** (다른 태스크 실행 가능)
5. HTTP 응답 도착 → oneshot으로 결과 전송
6. 봇 핸들러의 oneshot rx가 깨어남

Mutex 없이 채널만으로 동시성 확보.

---

## CART 마켓 (수동 관리)

시세 자동 조회가 불가능한 자산(비상장주식, 펀드, 크립토, 실물자산 등)을 포트폴리오에 수동 관리.

### 특성
- `Market::is_open_now()` → `false` (스케줄러·시그널에서 자동 제외)
- API 호출 없음 → `cached_price`를 현재가로 직접 사용
- 시그널 설정 불허 (자동 시세 조회 불가)
- 포맷: 원화 기준 (KRX와 동일)

### 명령어
```
/port add CART 비트코인 2 50000000 @코인 =55000000
/port edit 비트코인 2 50000000 =55000000 @코인
```
- `이름 = 종목코드` (단일 토큰)
- `=현재가`: 프리픽스로 현재가 지정 (CART 전용). `@계좌`와 순서 자유.
- `=현재가` 생략 시 매입가를 현재가로 사용

---

## 시그널 판정 로직

**방식**: REST 폴링. WebSocket 미사용 (종목 50개 미만, 5분 주기면 rate limit 여유 충분).

### 전체 흐름

```
스케줄러 (5분 interval)
  → is_market_hours()? (KRX 09:00~15:30 / US 22:30~05:00 KST)
  → check_all_signals()
       1. holdings + signals 테이블 로드 (SQLite)
       2. 활성(active=true) 시그널만 순회
       3. 포트폴리오에서 종목의 Market 파악 (없으면 스킵)
       4. market.is_open_now()? 장외면 스킵
       5. API Actor에 현재가 요청 (mpsc/oneshot)
       6. cached_price/cached_at 갱신
       7. 조건 평가 → 발동 시 텔레그램 전송 + active=false SQLite 저장
```

### 조건별 판정

| 조건 | 판정 로직 |
|------|-----------|
| `price_above` | `current_price >= target` |
| `price_below` | `current_price <= target` |
| `profit_above` | `(current - avg) / avg * 100 >= percentage` |
| `profit_below` | `(current - avg) / avg * 100 <= percentage` |

### 발동 후 처리

- **1회성**: 텔레그램 전송 성공 시에만 `active: false` → SQLite 저장
- 전송 실패 시 `active` 유지 → 다음 주기에 재시도
- 한 주기 내 여러 시그널 발동 가능 → 변경분 한 번에 저장

---

## 평가금액 계산

공통 공식: `price × quantity × value_factor`

| 마켓 | factor | 평가금액 (원화) | 예시 |
|------|--------|-----------------|------|
| KRX | 1.0 | `price × qty` | 72,500 × 10 = 725,000원 |
| NAS/NYS/AMS | 1.0 | `price × qty × usd_krw` | $195.20 × 5 × 1,450 = 1,415,200원 |
| BOND | 0.1 | `price × qty × 0.1` | 7,485 × 50,000 × 0.1 = 37,425,000원 |
| CART | 1.0 | `price × qty` | 55,000,000 × 2 = 110,000,000원 |

- **BOND**: 가격이 액면 10,000원 기준, 수량이 1,000원 단위 → `× 0.1` 보정 (`Market::value_factor()`)
- **해외주식**: `eval`은 달러 단위로 계산 후, 총합 합산 시 `× usd_krw`
- **손익**: `(현재가 - 매입가) × qty × factor` (해외주식은 추가로 `× usd_krw`)

---

## 워치리스트 (US 소형주 디스커버리)

### 개요
미국 소형주 관심종목 자동 발굴/평가 시스템. Google AI Studio API (Flash Lite + Gemma) + 한투 API 조합.

### 사냥 사이클 (`pipeline::run_cycle`)

```
스케줄러 진입 조건: discovery_enabled && prompts_configured && !hunt_exhausted && !judge_exhausted

0. cleanup_old_data(retention_days)        ← 오래된 DB 레코드 정리

1. 사냥 — gemini::hunt() [Flash Lite, 1콜]
   ├─ hunt 한도 체크 (max_hunt_calls_per_day)    ← API 호출 직전 본체 체크
   ├─ 프롬프트 + BL 목록 조합 → Flash Lite API 호출
   ├─ log_api_call("gemini", "hunt") 기록
   ├─ JSON 파싱 → HuntResult[] (ticker, market, name, sector, reason)
   ├─ prompt_history 저장
   └─ BL 필터링 → candidates(status=pending) 삽입

2. 수집 — 한투 API [N건, 순차]
   ├─ pending 후보 전부 조회
   ├─ fetch_detail(ticker, market_hint) → NAS→NYS→AMS 순회
   ├─ 성공: format_detail_for_gemini() → status=collected + detail_text 저장
   └─ 실패(3개 거래소 전부): 자동 BL 등록 + status=blacklisted

3. 평가 — gemini::judge() [Gemma, 1콜]
   ├─ judge 한도 체크 (max_judge_calls_per_day)   ← API 호출 직전 본체 체크
   ├─ collected 전체의 detail_text 합쳐서 Gemma API 호출
   ├─ log_api_call("gemini", "judge") 기록
   ├─ JSON 파싱 → JudgeResult[] (ticker, score, verdict)
   ├─ score < min_score → BL + status=blacklisted (처단)
   └─ score >= min_score → status=judged (생존)

4. 도태 — cull_excess_judged(max_survivors)
   └─ judged 상위 N개만 유지, 나머지 삭제
```

### 점수 계산

2단계 가중 합산: 사냥 매력(hunt 1차 인상평) + 가죽 품질(judge 시장 데이터 기반)

```
최종 등급 = 사냥 매력 × w + 가죽 품질 × (1 - w)    (w = hunt_weight, 0.0~1.0)
```

| hunt_weight | 비율 | 설명 |
|---|---|---|
| 0.0 | 사냥 매력 0% : 가죽 품질 100% | 가죽 품질만 반영 |
| 0.3 | 사냥 매력 30% : 가죽 품질 70% | 가죽 품질 비중 높음 |
| 0.5 | 사냥 매력 50% : 가죽 품질 50% | 동등 (기본값) |
| 0.7 | 사냥 매력 70% : 가죽 품질 30% | 사냥 매력 비중 높음 |
| 1.0 | 사냥 매력 100% : 가죽 품질 0% | 사냥 매력만 반영 |

- `사냥 매력` (hunt_score): 사냥 시 LLM이 프롬프트 기준으로 매긴 확신도 (0–100)
- `가죽 품질` (judge_score): 수집된 시장 데이터(PER, PBR, EPS 등) 기반 평가 (0–100)
- `min_score` 비교는 최종 등급 기준

### 재평가 사이클 (`pipeline::run_reeval`)

KST 02:00, 하루 1회 자동 실행. 스케줄러 진입 조건: `!judge_exhausted()`

```
0. revive_near_misses() → 패자 부활 (점수 아깝게 떨어진 BL → pending 복귀)
   - 부활 조건: score >= min_score * 0.9 AND strike_count < 3
   - 부활 시 BL 삭제 + candidate pending 리셋
   - API 조회 실패 BL / 수동 BL은 부활 불가 (score 없음)
1. reset_judged_for_reeval() → 기존 judged 전부 → pending 리셋
2. 재수집 (사냥 사이클 2단계와 동일)
3. 재평가 (사냥 사이클 3단계와 동일)
4. 도태 (사냥 사이클 4단계와 동일)
```

### 후보 상태 전이

```
pending → collected → judged (생존)
                   → blacklisted (처단: score < min_score)
pending → blacklisted (수집 실패 → 영구 BL)
judged → pending (재평가 리셋) → ...반복
blacklisted → pending (패자 부활: score >= min_score*0.9 && strike < 3)
```

### 한도 체크 구조

API 호출 직전 본체 체크 + 스케줄러 진입 시 사전필터 (이중 체크):

| API | 본체 (gemini.rs) | 사전필터 (scheduler.rs) | 기본 한도 |
|-----|-----------------|----------------------|-----------|
| hunt (Flash Lite) | `hunt()` 진입부 | 사냥 사이클 진입 시 | 20/일 |
| judge (Gemma) | `judge()` 진입부 | 사냥/재평가 사이클 진입 시 | 14,400/일 |

사용량 추적: `api_usage` 테이블에 `log_api_call()` → 성공/실패 구분 없이 전체 카운트.
리셋: `called_at LIKE '{today}%'` 날짜 매칭으로 자연 리셋. 기준 시간대는 태평양시간(고정 UTC-8, 서머타임 무시) — Google AI Studio 일일 쿼터 리셋과 동기화.

### 모델 구성

| 용도 | config 키 | 기본값 | 비고 |
|------|-----------|--------|------|
| 사냥 | `hunt_models` | ["gemini-2.5-flash-lite"] | 배열 순서대로 폴백 |
| 평가 | `judge_models` | ["gemma-3-27b-it"] | 배열 순서대로 폴백 |

API 호출 실패 시 다음 모델로 자동 폴백. 당일 성공한 모델을 우선 사용하며 다음날(태평양시간 자정) 리셋.

둘 다 Google AI Studio API (`generativelanguage.googleapis.com`)를 `gemini_api_key` 하나로 호출.
Gemini 호출은 API Actor를 거치지 않음 (별도 `reqwest::Client` 직접 사용).

### 프롬프트 관리
- **사냥용 (hunt)**: 종목 발굴 기준. 사용자 설정 필수.
- **평가용 (judge)**: 평가 기준. 사용자 설정 필수.
- 기본 프롬프트 없음 — 미설정 시 디스커버리 불가.
- DB `prompts` 테이블에 저장, 텔레그램에서 실시간 수정 가능.

### 데이터 저장
- **SQLite** (`/opt/kkuepark/gidbcn/portfolio.db`, WAL 모드)
- `thread_local! RefCell<Option<Connection>>` — LocalSet 덕에 `!Send` OK
- 테이블: candidates, blacklist, prompts, prompt_history, api_usage, holdings, signals

### 한투 API 확장
- `OverseasDetail` 구조체: 기존 PriceData + 시총, PER, PBR, EPS, BPS, 거래량, 52주고저, 섹터
- `overseas::get_detail()`: 동일 엔드포인트(HHDFS76200200)에서 전체 필드 파싱
- `ApiRequest::GetOverseasDetail` + `ApiHandle::get_overseas_detail()`
