# 구현 아키텍처

## 봇 명령 처리 흐름

텔레그램 명령 수신부터 파일 저장까지의 흐름:

```
사용자 텔레그램 메시지
  → teloxide 디스패처 (handler.rs)
  → handle_command() (commands.rs)
       │
       ├─ [포트폴리오 조회 없는 명령: /add, /remove, /edit, /signal*]
       │    storage::load_*()          // std::fs로 JSON 읽기
       │    데이터 변경
       │    storage::save_*()          // std::fs로 JSON 쓰기
       │    응답 문자열 반환
       │
       └─ [현재가 필요한 명령: /list, /info, /summary]
            storage::load_portfolio()
            api.get_price_for_market() ──► API Actor (mpsc)
                                          reqwest → 한투 API
                                      ◄── PriceData (oneshot)
            formatter로 메시지 생성
            [이름 캐싱] storage::save_portfolio()  // name 비어있던 경우만
            응답 문자열 반환
  → bot.send_message()
```

**JSON 파일 경로**: `/opt/kkuepark/gidbcn/portfolio_{user_id}.json`, `signals_{user_id}.json`
**동기 I/O**: `std::fs` 직접 사용 (수 KB 파일, current_thread에서 블로킹 무시 가능)

---

## 스케줄 작업

`tokio::select!`로 두 interval을 동시 대기:

```
minute_tick (1분마다)
  → 현재 KST 시각 확인
  → 08:50 또는 15:40이면: api.get_exchange_rate() → config.json 갱신

signal_tick (기본 5분마다)
  → is_market_hours()?
      KRX: 09:00~15:30 KST
      US:  22:30~05:00 KST
  → 장중 아니면 스킵
  → storage::list_user_ids()로 사용자 목록 조회
  → 각 사용자별 check_all_signals()
       활성 시그널 순회 → 현재가 조회 → 조건 평가
       발동 시: 텔레그램 알림 전송 + active=false 저장
```

**토큰 갱신**: API Actor가 매 요청 전 `token_needs_refresh()`를 확인. 만료 1시간 전이면 자동 갱신 후 `config.json` 저장.

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
       ├─ mpsc.send(ApiRequest { symbol, respond_to: tx }) ──►  API Actor
       │                                                          ├─ rate_limit() (50ms)
       │                                                          ├─ reqwest GET → 한투 API
       ◄── rx.await (응답 대기)  ◄── respond_to.send(Result) ────┘

Fire-and-forget (토큰 갱신):

  Bot Task ──[RefreshToken]──► API Actor   (respond_to 없음)
```

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
}
```

호출하는 쪽은 내부 채널을 모름. 그냥 `api.get_domestic_price("005930").await?` 하면 됨.

### 메시지 타입

```rust
pub enum ApiRequest {
    GetDomesticPrice   { symbol, respond_to: oneshot::Sender<Result<PriceData>> },
    GetOverseasPrice   { exchange, symbol, respond_to: ... },
    GetBondPrice       { isin, respond_to: ... },
    GetExchangeRate    { respond_to: ... },
    RefreshToken,      // fire-and-forget
}
```

### API Actor 내부

`ActorContext`가 reqwest::Client, 토큰, rate limiter를 독점 소유. 외부 접근 불가.

```rust
pub async fn run_api_actor(mut rx: mpsc::Receiver<ApiRequest>, config: Config) {
    let mut ctx = ActorContext::new(config.kis_api.clone());

    while let Some(req) = rx.recv().await {
        // 매 요청마다 토큰 만료 체크
        if auth::token_needs_refresh(&ctx.config.token) {
            ctx.refresh_token(&mut config).await;
        }

        match req {
            ApiRequest::GetDomesticPrice { symbol, respond_to } => {
                ctx.rate_limit().await;   // 50ms 간격 보장
                let result = domestic::get_price(&ctx, &symbol).await;
                let _ = respond_to.send(result);
            }
            // ... 나머지 동일 패턴
        }
    }
}
```

- `rate_limit()`: 마지막 요청 이후 50ms 미경과 시 sleep. 초당 20회 제한 준수.
- 모든 tx가 drop되면 `rx.recv()` → `None` → 루프 종료.

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

## 시그널 판정 로직

**방식**: REST 폴링. WebSocket 미사용 (종목 50개 미만, 5분 주기면 rate limit 여유 충분).

### 전체 흐름

```
스케줄러 (5분 interval)
  → is_market_hours()? (KRX 09:00~15:30 / US 22:30~05:00 KST)
  → check_all_signals()
       1. portfolio.json + signals.json 로드
       2. 활성(active=true) 시그널만 순회
       3. 포트폴리오에서 종목의 Market 파악 (없으면 스킵)
       4. API Actor에 현재가 요청 (mpsc/oneshot)
       5. 조건 평가 → 발동 시 텔레그램 전송 + active=false 저장
```

### 조건별 판정

| 조건 | 판정 로직 |
|------|-----------|
| `price_above` | `current_price >= target` |
| `price_below` | `current_price <= target` |
| `profit_above` | `(current - avg) / avg * 100 >= percentage` |
| `profit_below` | `(current - avg) / avg * 100 <= percentage` |

### 발동 후 처리

- **1회성**: 발동 즉시 `active: false` → signals.json 저장. 재발동하려면 사용자가 다시 설정.
- 한 주기 내 여러 시그널 발동 가능 → 변경분 한 번에 저장.

---

## 스케줄러 상세

`tokio::select!`로 두 개의 interval을 동시 대기:

- **signal_tick** (설정값, 기본 5분): `is_market_hours()` 통과 시 `check_all_signals()` 실행
- **minute_tick** (1분): 환율 갱신 시간(08:50, 15:40 KST) 확인. 중복 방지를 위해 `last_exchange_update` 추적.

### 장시간 판단

```rust
fn is_market_hours() -> bool {
    let hhmm = hour * 100 + min;  // KST 기준
    let krx_open = hhmm >= 900 && hhmm <= 1530;
    let us_open = hhmm >= 2230 || hhmm <= 500;
    krx_open || us_open
}
```

KRX와 US 장 시간대 중 하나라도 열려있으면 체크 실행. 장외 시간에는 불필요한 API 호출 안 함.
