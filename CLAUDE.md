# CLAUDE.md — gidbcn (Finance Signal Alert)

## 프로젝트 개요
금융 시그널 포착 및 텔레그램 알림 시스템. (레포: `gidbcn`)
사용자가 텔레그램 봇 명령어로 포트폴리오를 관리하고, 한투 Open API로 시세를 주기적으로 조회하여, 사전 설정한 시그널 조건 충족 시 텔레그램으로 알림을 보낸다.

**핵심 제약**: 한투 계좌 연동 없음. 시세 데이터 조회 용도로만 사용.
**배포 타겟**: 라즈베리파이 2B+ (ARMv7, 1GB RAM)

---

## 아키텍처 — Mutex-Free Actor 패턴

### 런타임
- **`tokio::runtime::Builder::new_current_thread()`** — 싱글스레드 이벤트 루프
- GLib `GMainContext` 철학: 같은 스레드 내에서 순차 실행, 동시 접근 없음
- **애플리케이션 코드에서 Mutex, RwLock, Arc<Mutex<T>> 사용 금지** (0개)
- 파일 I/O: `std::fs` 사용 (수 KB 파일, 블로킹 무시 가능). `tokio::fs` 미사용 (`spawn_blocking` 방지)

### Actor 구조 (2개)

```
┌─────────────────────────────────────────────────────┐
│  tokio current_thread runtime                       │
│                                                     │
│  ┌───────────────────────┐  mpsc   ┌─────────────┐ │
│  │      Bot Task          │◄───────►│  API Actor   │ │
│  │  (메인 태스크)          │ oneshot │ (reqwest,    │ │
│  │                        │         │  token,      │ │
│  │  • teloxide 디스패처    │         │  rate limit) │ │
│  │  • 명령어 처리          │         └─────────────┘ │
│  │  • 시그널 엔진          │                         │
│  │  • 스케줄러 (interval)  │  직접 호출 (sync)       │
│  │  • JSON I/O 직접 처리   │◄──────► data/*.json     │
│  └───────────────────────┘                          │
└─────────────────────────────────────────────────────┘
```

- **Bot Task**: teloxide 디스패처 + 명령어 처리 + 시그널 엔진 + 스케줄러 + JSON I/O
- **API Actor**: reqwest::Client, access_token, rate limiter 독점 소유. mpsc로 요청 수신, oneshot으로 응답 반환

### Actor 간 통신
```rust
// 메시지 enum (src/models/messages.rs)
enum ApiRequest {
    GetDomesticPrice { symbol: String, respond_to: oneshot::Sender<Result<PriceData>> },
    GetOverseasPrice { exchange: String, symbol: String, respond_to: oneshot::Sender<Result<PriceData>> },
    GetDailyChart { market: Market, symbol: String, respond_to: oneshot::Sender<Result<Vec<DailyCandle>>> },
    GetBondPrice { isin: String, respond_to: oneshot::Sender<Result<BondData>> },
    GetExchangeRate { respond_to: oneshot::Sender<Result<f64>> },
    RefreshToken, // fire-and-forget
}
```

### ApiHandle 패턴
```rust
// API Actor에 접근하는 핸들 (Clone 가능)
#[derive(Clone)]
struct ApiHandle {
    sender: mpsc::Sender<ApiRequest>,
}

impl ApiHandle {
    async fn get_domestic_price(&self, symbol: &str) -> Result<PriceData> {
        let (tx, rx) = oneshot::channel();
        self.sender.send(ApiRequest::GetDomesticPrice {
            symbol: symbol.to_string(),
            respond_to: tx,
        }).await?;
        rx.await?
    }
    // ... 각 API에 대해 동일 패턴
}
```

---

## 프로젝트 구조

```
gidbcn/
├── Cargo.toml
├── CLAUDE.md              ← 이 파일
├── .gitignore
├── data/
│   ├── config.json        ← gitignore (API 키, 토큰)
│   ├── portfolio.json
│   ├── signals.json
│   └── alert_log.json
└── src/
    ├── main.rs            ← 채널 생성 → API Actor spawn → Bot 실행
    ├── config.rs          ← Config 구조체, JSON 로드/저장
    ├── api/
    │   ├── mod.rs
    │   ├── actor.rs       ← ApiActor 수신 루프 + ApiHandle
    │   ├── auth.rs        ← OAuth 토큰 발급/갱신
    │   ├── domestic.rs    ← 국내주식 현재가 + 일봉
    │   ├── overseas.rs    ← 해외주식 현재가 + 일봉
    │   ├── bond.rs        ← 국내채권 현재가
    │   └── exchange.rs    ← 환율 조회
    ├── bot/
    │   ├── mod.rs
    │   ├── handler.rs     ← teloxide 디스패처 설정
    │   ├── commands.rs    ← /add, /remove, /list 등
    │   └── formatter.rs   ← 텔레그램 메시지 포맷
    ├── signal/
    │   ├── mod.rs
    │   ├── engine.rs      ← 시그널 조건 평가 엔진
    │   ├── price.rs       ← price_above/below, profit_above/below
    │   ├── technical.rs   ← golden_cross, dead_cross, RSI
    │   └── volume.rs      ← volume_surge
    ├── storage.rs         ← portfolio/signals/alert_log JSON CRUD (동기)
    ├── scheduler.rs       ← tokio::time::interval 기반
    └── models/
        ├── mod.rs
        ├── portfolio.rs   ← Holding
        ├── signal.rs      ← Signal, Condition
        ├── alert.rs       ← AlertLog
        └── messages.rs    ← ApiRequest enum
```

---

## main.rs 초기화 흐름

```rust
#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::init();

    // 1. config 로드
    let config = Config::load("data/config.json");

    // 2. API Actor 채널 생성 + spawn
    let (api_tx, api_rx) = mpsc::channel::<ApiRequest>(32);
    let api_handle = ApiHandle { sender: api_tx };
    tokio::spawn(run_api_actor(api_rx, config.kis_api.clone()));

    // 3. 스케줄러 spawn
    tokio::spawn(run_scheduler(api_handle.clone(), config.scheduler.clone()));

    // 4. 텔레그램 봇 실행 (메인 태스크, block)
    run_bot(config.telegram, api_handle).await;
}
```

---

## 한투 Open API 상세

### 공통
- Base URL (실전): `https://openapi.koreainvestment.com:9443`
- 인증: Bearer token + appkey + appsecret 헤더
- **초당 20회 제한** (전체 API 합산)
- 토큰 발급: **1분당 1회**, 유효기간 24시간

### 공통 헤더
```
Content-Type: application/json; charset=utf-8
authorization: Bearer {access_token}
appkey: {APP_KEY}
appsecret: {APP_SECRET}
tr_id: {거래ID}
```

### 엔드포인트

#### 0. 토큰 발급
- POST `/oauth2/tokenP`
- Body: `{ "grant_type": "client_credentials", "appkey": "...", "appsecret": "..." }`
- 응답: `access_token`, `expires_in` (86400초)

#### 1. 국내주식 현재가
- GET `/uapi/domestic-stock/v1/quotations/inquire-price`
- tr_id: `FHKST01010100`
- Query: `FID_COND_MRKT_DIV_CODE=J`, `FID_INPUT_ISCD={종목코드6자리}`
- 응답: `stck_prpr`(현재가), `prdy_vrss`(전일대비), `prdy_ctrt`(등락률), `acml_vol`(거래량)

#### 2. 국내주식 일봉
- GET `/uapi/domestic-stock/v1/quotations/inquire-daily-itemchartprice`
- tr_id: `FHKST03010100`
- Query: `FID_COND_MRKT_DIV_CODE=J`, `FID_INPUT_ISCD`, `FID_INPUT_DATE_1`, `FID_INPUT_DATE_2`, `FID_PERIOD_DIV_CODE=D`
- 응답: `stck_clpr`(종가), `stck_oprc`(시가), `stck_hgpr`(고가), `stck_lwpr`(저가), `acml_vol`(거래량), `stck_bsop_date`(일자)

#### 3. 해외주식 현재가
- GET `/uapi/overseas-price/v1/quotations/price`
- tr_id: `HHDFS00000300`
- Query: `AUTH=""`, `EXCD={NAS|NYS|AMS}`, `SYMB={티커}`
- 응답: `last`(현재가), `diff`(전일대비), `rate`(등락률), `tvol`(거래량), `name`(종목명)

#### 4. 해외주식 일봉
- GET `/uapi/overseas-price/v1/quotations/dailyprice`
- tr_id: `HHDFS76240000`
- Query: `AUTH=""`, `EXCD`, `SYMB`, `GUBN=0`(일), `BYMD={YYYYMMDD}`, `MODP=1`(수정주가)
- 응답: `clos`(종가), `open`(시가), `high`(고가), `low`(저가), `tvol`(거래량), `xymd`(일자)

#### 5. 국내채권 현재가
- GET `/uapi/domestic-bond/v1/quotations/inquire-price`
- tr_id: `FHKBJ773000C0`
- Query: `FID_COND_MRKT_DIV_CODE=B`, `FID_INPUT_ISCD={ISIN 12자리}`

#### 6. 환율 조회
- GET `/uapi/overseas-stock/v1/quotations/inquire-exchange-rate`
- 하루 2회 캐싱 (08:50, 15:40) → config.json의 exchange_rate에 저장

---

## JSON 스키마

### config.json (gitignore)
```json
{
  "kis_api": {
    "app_key": "...",
    "app_secret": "...",
    "base_url": "https://openapi.koreainvestment.com:9443",
    "hts_id": "myid01",
    "token": {
      "access_token": "...",
      "expires_at": "2026-02-27T14:30:00+09:00"
    }
  },
  "telegram": {
    "bot_token": "123456789:ABCdef...",
    "chat_id": 123456789
  },
  "exchange_rate": {
    "usd_krw": 1350.50,
    "updated_at": "2026-02-26T15:40:00+09:00"
  },
  "scheduler": {
    "signal_check_interval_minutes": 5,
    "exchange_rate_cron": ["0 50 8 * * *", "0 40 15 * * *"]
  }
}
```

### portfolio.json
```json
{
  "next_id": 4,
  "holdings": [
    {
      "id": "h_001",
      "market": "KRX",
      "symbol": "005930",
      "quantity": 10,
      "avg_price": 70000.0,
      "added_at": "2026-02-26T10:00:00+09:00"
    }
  ]
}
```
- market: `KRX` | `NAS` | `NYS` | `AMS` | `BOND`
- ID: 자동 증가 (h_001, h_002, ...)
- 종목명: 저장 안 함 (한투 API 응답에서 가져옴)

### signals.json
```json
{
  "next_id": 3,
  "signals": [
    {
      "id": "s_001",
      "symbol": "005930",
      "condition": {
        "type": "price_above",
        "params": { "target": 80000.0 }
      },
      "active": true,
      "created_at": "2026-02-26T10:30:00+09:00"
    }
  ]
}
```

**condition types:**
| type | params | 설명 |
|---|---|---|
| `price_above` | `{ target: f64 }` | 현재가 ≥ target |
| `price_below` | `{ target: f64 }` | 현재가 ≤ target |
| `profit_above` | `{ percentage: f64 }` | 수익률 ≥ %  (매입가 대비) |
| `profit_below` | `{ percentage: f64 }` | 수익률 ≤ % |
| `golden_cross` | `{ short_period: u32, long_period: u32 }` | 단기MA 상향돌파 |
| `dead_cross` | `{ short_period: u32, long_period: u32 }` | 단기MA 하향돌파 |
| `rsi_above` | `{ threshold: f64 }` | RSI ≥ threshold |
| `rsi_below` | `{ threshold: f64 }` | RSI ≤ threshold |
| `volume_surge` | `{ threshold_pct: f64 }` | 거래량 ≥ 20일평균 × pct% |

**시그널 동작**: 1회성 발동 → 자동 비활성화 (active: false). 재설정 필요.

### alert_log.json
```json
{
  "next_id": 2,
  "alerts": [
    {
      "id": "a_001",
      "signal_id": "s_001",
      "symbol": "005930",
      "condition_type": "price_above",
      "trigger_value": 80500.0,
      "message": "🚨 시그널 발동! ...",
      "sent_at": "2026-02-26T13:45:00+09:00",
      "success": true
    }
  ]
}
```

---

## 텔레그램 봇 명령어

### 포트폴리오 관리
| 명령어 | 설명 |
|---|---|
| `/add [마켓] [종목코드] [수량] [매입가]` | 종목 추가 |
| `/remove [종목코드]` | 종목 삭제 |
| `/edit [종목코드] [수량] [매입가]` | 종목 수정 |
| `/list` | 전체 포트폴리오 + 현재가 + 손익 |
| `/info [종목코드]` | 개별 종목 상세 + 시그널 |
| `/summary` | 자산배분 요약 + 총 손익 |

### 시그널 관리
| 명령어 | 설명 |
|---|---|
| `/signal [종목코드] [조건타입] [파라미터...]` | 시그널 설정 |
| `/signal_list` | 전체 시그널 조회 |
| `/signal_remove [시그널ID]` | 시그널 삭제 |
| `/signal_clear [종목코드]` | 종목 시그널 전체 삭제 |

### 시스템
| 명령어 | 설명 |
|---|---|
| `/status` | 시스템 상태 |
| `/help` | 도움말 |

### 마켓 코드 매핑
| 텔레그램 | 한투 API | 비고 |
|---|---|---|
| `KRX` | `FID_COND_MRKT_DIV_CODE=J` | 주식/ETF |
| `NAS` | `EXCD=NAS` | 나스닥 |
| `NYS` | `EXCD=NYS` | 뉴욕 |
| `AMS` | `EXCD=AMS` | 아멕스 |
| `BOND` | `FID_COND_MRKT_DIV_CODE=B` | 채권 별도 엔드포인트 |

---

## 스케줄러

`tokio::time::interval` 기반. `tokio-cron-scheduler` 미사용.

| 작업 | 주기 | 조건 |
|---|---|---|
| 시그널 체크 (현재가) | 5분 | 장중에만 (KRX: 09:00~15:30, US: 22:30~05:00 KST) |
| 일봉 수집 + 기술적 시그널 | 1일 1회 | KRX 장 마감 후 16:00, US 장 마감 후 06:00 KST |
| 환율 조회 | 1일 2회 | 08:50, 15:40 |
| 토큰 갱신 | 만료 1시간 전 | `expires_at` 기준 자동 판단 |

---

## 의존성 (Cargo.toml)

```toml
[package]
name = "gidbcn"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { version = "1", features = ["rt", "macros", "time", "sync"] }
reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }
teloxide = { version = "0.13", features = ["macros"] }
tracing = "0.1"
tracing-subscriber = "0.3"
```

---

## 코딩 규칙

1. **Mutex/RwLock/Arc<Mutex<T>> 절대 사용 금지**. Actor 패턴 + 채널로만 통신.
2. **`tokio::fs` 사용 금지**. `std::fs`로 동기 I/O (수 KB 파일, `spawn_blocking` 방지).
3. **`tokio-cron-scheduler` 사용 금지**. `tokio::time::interval`로 직접 구현.
4. **에러 처리**: `anyhow` 또는 커스텀 Error enum. `unwrap()` 최소화.
5. **로깅**: `tracing` 매크로 (`info!`, `warn!`, `error!`) 사용.
6. **API 응답 필드**: 한투 API는 모든 숫자를 String으로 반환 → 파싱 필요.
7. **TLS**: rustls 사용 (크로스 컴파일 용이). OpenSSL 의존 금지.
8. **rate limiting**: API Actor 내부에서 `tokio::time::sleep`으로 초당 20회 제한 준수.

---

## 검증 필요 사항 (PoC)

- **teloxide + `current_thread` 호환성**: GitHub 이슈 #366. 구현 초기에 최소 PoC (봇 시작 + 명령어 1개 응답) 검증. 문제 시 대안: `reqwest`로 Telegram Bot API 직접 호출 + long polling 자체 구현.

---

## 알림 메시지 형식 예시

```
🚨 시그널 발동!
005930 삼성전자
조건: 가격 ≥ 80,000
현재가: 80,500원 (+1.2%)
시간: 2026-02-26 13:45

💡 매입가: 70,000원 | 수익률: +15.0%
```

---

## 조회 명령어 출력 예시

### /list
```
📊 포트폴리오 현황
2026-02-26 14:30 기준

🇰🇷 국내
• 005930 삼성전자 | 10주 | 70,000→72,500 | +3.6%
• 069500 KODEX200 | 100주 | 35,000→34,200 | -2.3%

🇺🇸 미국
• TSLA 테슬라 | 5주 | $180.50→$195.20 | +8.1%

🏛 채권
• KR103502G9C8 | 10 | 9,850→9,920 | +0.7%

──────────
💰 총 평가: 45,234,500원
💵 총 손익: +1,234,500원 (+2.8%)
```

### /info [종목코드]
```
📈 005930 삼성전자
현재가: 72,500원 (전일 대비 +1.2%)
매입가: 70,000원 × 10주
평가금액: 725,000원
손익: +25,000원 (+3.6%)

⚡ 설정된 시그널:
• 가격 ≥ 80,000 → 알림
• RSI ≤ 30 → 알림
```

### /summary
```
📊 포트폴리오 요약
🇰🇷 국내: 4,145,000원 (38%)
🇺🇸 미국: 5,832,000원 (54%)
🏛 채권: 992,000원 (8%)
──────────
💰 총 평가: 10,969,000원
💵 총 손익: +569,000원 (+5.5%)
```
