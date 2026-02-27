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
│  │  • JSON I/O 직접 처리   │◄──────► /opt/.../portfolio.json │
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
    GetBondPrice { isin: String, respond_to: oneshot::Sender<Result<BondData>> },
    GetExchangeRate { respond_to: oneshot::Sender<Result<f64>> },
    GetStockName { prdt_type_cd: String, pdno: String, respond_to: oneshot::Sender<Result<String>> },
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
├── Makefile               ← 크로스컴파일 + 배포 (make build-pi, make deploy)
├── .gitignore
├── .cargo/
│   └── config.toml        ← armv7 링커 설정, release strip
├── docs/
│   ├── architecture.md
│   ├── build.md           ← 크로스컴파일 가이드
│   └── config.template.json
└── src/
    ├── main.rs            ← 채널 생성 → API Actor spawn → Bot 실행
    ├── config.rs          ← Config 구조체, JSON 로드/저장
    ├── api/
    │   ├── mod.rs
    │   ├── actor.rs       ← ApiActor 수신 루프 + ApiHandle
    │   ├── auth.rs        ← OAuth 토큰 발급/갱신
    │   ├── domestic.rs    ← 국내주식 현재가 (inquire-price, 날짜 파라미터 없음)
    │   ├── overseas.rs    ← 해외주식 현재가 (t_rate 부산물로 환율 갱신)
    │   ├── bond.rs        ← 국내채권 현재가
    │   └── stock_info.rs  ← 종목명 조회 (상품기본조회 CTPF1604R)
    ├── bot/
    │   ├── mod.rs
    │   ├── handler.rs     ← teloxide 디스패처 설정 (비커맨드 메시지 default_handler로 무시)
    │   ├── commands.rs    ← /port, /signal 서브커맨드
    │   └── formatter.rs   ← 텔레그램 메시지 포맷
    ├── signal/
    │   ├── mod.rs
    │   ├── engine.rs      ← 시그널 조건 평가 엔진
    │   └── price.rs       ← price_above/below, profit_above/below
    ├── storage.rs         ← portfolio/signals JSON CRUD (동기)
    ├── scheduler.rs       ← tokio::time::interval 기반
    └── models/
        ├── mod.rs
        ├── portfolio.rs   ← Holding
        ├── signal.rs      ← Signal, Condition
        ├── alert.rs       ← AlertLog
        └── messages.rs    ← ApiRequest enum
```

**데이터 파일 경로** (레포 외부): `/opt/kkuepark/gidbcn/`
- `config.json` — API 키, 토큰, 허용 사용자 목록 (gitignore)
- `portfolio.json` — 전체 사용자 포트폴리오 (user_id 키 통합)
- `signals.json` — 전체 사용자 시그널 (user_id 키 통합)

---

## main.rs 초기화 흐름

```rust
#[tokio::main(flavor = "current_thread")]
async fn main() {
    // 1. config 로드 (로깅 초기화 전 — 오류는 eprintln 사용)
    //    없으면 템플릿 생성 후 종료 / 파싱 실패 시 오류 출력
    let config = Config::load(storage::CONFIG_PATH)?;

    // 1-1. log 섹션 등 신규 섹션 누락 시 defaults 포함해서 자동 저장 (마이그레이션)

    // 2. 필수 설정 검증: bot_token, app_key, app_secret, hts_id 누락 시 종료
    //    (값이 비어있거나 "YOUR_"로 시작하면 미설정으로 판단)

    // 3. 로깅 초기화 (config.log 기준)
    //    - stdout: RUST_LOG 환경변수 기준
    //    - 파일: config.log.dir/gidbcn.YYYY-MM-DD.log, WARN 이상만, config.log.retain_days일 보관

    // 4. API Actor 채널 생성 + spawn
    let (api_tx, api_rx) = mpsc::channel::<ApiRequest>(32);
    let api_handle = ApiHandle::new(api_tx);
    tokio::spawn(run_api_actor(api_rx, config.clone()));

    // 5. 스케줄러 spawn (bot 인스턴스 공유)
    let tg_bot = Bot::new(&config.telegram.bot_token);
    tokio::spawn(run_scheduler(api_handle.clone(), config.scheduler.clone(), tg_bot.clone()));

    // 6. 텔레그램 봇 실행 (메인 태스크, block)
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

### reqwest 클라이언트 설정 (ActorContext)
| 설정 | 값 | 이유 |
|---|---|---|
| `connect_timeout` | 5s | TCP+TLS 연결 수립 타임아웃 |
| `timeout` | 15s | 전체 요청(연결~응답 수신) 타임아웃 |
| `pool_idle_timeout` | 55s | 1차 방어: KIS API 서버 idle timeout보다 짧게 설정하여 stale connection 사전 제거 |

#### stale connection 재시도 (`ActorContext::send_with_retry`)
HTTP keep-alive 연결을 서버가 종료한 뒤 클라이언트가 재사용 시도 시 "connection closed before message completed" 발생. `pool_idle_timeout`만으로는 타이밍 race condition을 완전히 막을 수 없으므로 모든 API 함수는 `ctx.send_with_retry(builder)` 사용.
- 연결 오류(`is_request` / `is_connect`)에 한해 1회 재시도
- 타임아웃(`is_timeout`)은 재시도하지 않음 (대기 시간 2배 방지)
- 재시도 시 `DEBUG` 레벨 로그 출력

### 공통 헤더
```
Content-Type: application/json; charset=utf-8
authorization: Bearer {access_token}
appkey: {APP_KEY}
appsecret: {APP_SECRET}
tr_id: {거래ID}
custtype: P
```
- `custtype: P` — 국내주식·채권 전 API 필수(Y). 해외주식 현재가상세는 선택(N)이지만 항상 전송해도 무해. `common_headers()`에서 전 엔드포인트 공통 적용.

### 엔드포인트

#### 0. 토큰 발급
- POST `/oauth2/tokenP`
- Body: `{ "grant_type": "client_credentials", "appkey": "...", "appsecret": "..." }`
- 응답: `access_token`, `access_token_token_expired` ("YYYY-MM-DD HH:MM:SS" KST 형식)
- **주의**: `expires_in` 필드 없음. 만료시간은 `access_token_token_expired` 파싱. 파싱 실패 시 발급 시각 +24h로 fallback
- 갱신 조건: `expires_at - 1시간` 이전이면 자동 갱신 (`token_needs_refresh()`)

#### 1. 국내주식 현재가
- GET `/uapi/domestic-stock/v1/quotations/inquire-price`
- tr_id: `FHKST01010100`
- Query: `FID_COND_MRKT_DIV_CODE=J`, `FID_INPUT_ISCD={종목코드6자리}`
- 응답 (`output`): `stck_prpr`(현재가), `prdy_ctrt`(등락률)
- 종목명 미포함 → `Holding.name` 사용. 날짜 파라미터 없음 → 장 외 시간도 정상 동작

#### 2. 해외주식 현재가상세 (+ 환율)
- GET `/uapi/overseas-price/v1/quotations/price-detail`
- tr_id: `HHDFS76200200`
- Query: `AUTH=""`, `EXCD={NAS|NYS|AMS}`, `SYMB={티커}`
- 응답 (`output`) 주요 필드: `last`(현재가), `name`(종목명), `t_xrat`(원환산당일등락률%), `t_rate`(당일환율)
- **`rate` 필드 없음**. 등락률은 `t_xrat` 사용 (`name`은 존재하며 실제로 읽음)
- **`t_rate` 부산물 캐싱**: 해외주식 시세 조회마다 actor 로컬 변수 `usd_krw`에 자동 갱신 (파일 저장 없음)
- `GetExchangeRate` 메시지 → actor의 `usd_krw` 메모리 값 즉시 반환 (HTTP 호출 없음)

#### 3. 국내채권 현재가 (장내채권현재가(시세), 국내주식-200)
- GET `/uapi/domestic-bond/v1/quotations/inquire-price`
- tr_id: `FHKBJ773400C0` (모의투자 미지원)
- Query: `FID_COND_MRKT_DIV_CODE=B`, `FID_INPUT_ISCD={ISIN 12자리}` (예: `KR2033022D33`)
- 응답 (`output`) 주요 필드:
  - `hts_kor_isnm` — 종목명
  - `bond_prpr` — 채권현재가 (액면가 10,000원 기준 가격, 예: 10265.00)
  - `bond_prdy_vrss` — 전일대비
  - `prdy_ctrt` — 전일대비율(등락률)
  - `ernn_rate` — 현재수익률(YTM)
  - `acml_vol` — 누적거래량
- **채권 가격 단위**: `bond_prpr`는 액면가 10,000원 기준 가격 (예: 7,485 = 74.85%)
- **채권 수량 단위**: `quantity`는 액면가 1,000원 단위 수량 (예: 50,000 = 액면 5,000만원)
- **평가금액 계산**: `price × quantity × 0.1` → `Market::value_factor()` = 0.1 적용
- **주의**: `custtype: P` 헤더 필수. 미전송 시 요청 실패

#### 4. 종목명 조회 (상품기본조회)
- GET `/uapi/domestic-stock/v1/quotations/search-info`
- tr_id: `CTPF1604R`
- Query: `PRDT_TYPE_CD={코드}`, `PDNO={종목코드/ISIN}`
- 응답 (`output`): `prdt_abrv_name`(표시용 약어명) 사용
- `Market::product_type_code()`: KRX=300, NAS=512, NYS=513, AMS=529, BOND=302
- **전 마켓 단일 엔드포인트**. `/port add` 시 자동 조회 (조회 실패 시 add 거부)

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
    "owner_chat_id": 123456789,
    "users": [987654321]
  },
  "scheduler": {
    "signal_check_interval_minutes": 5
  },
  "log": {
    "dir": "/opt/kkuepark/gidbcn",
    "retain_days": 7
  }
}
```
- **환율 없음**: `usd_krw`는 config에 저장하지 않음. actor 시작 시 기본값 1350.0, 이후 해외주식 조회 시 t_rate로 자동 갱신.
- **log 섹션 자동 마이그레이션**: 기존 config.json에 `log` 키가 없으면 시작 시 defaults 포함해서 자동 저장.

### portfolio.json
```json
{
  "42621862": {
    "holdings": [
      {
        "market": "KRX",
        "symbol": "005930",
        "name": "삼성전자",
        "quantity": 10,
        "avg_price": 70000.0,
        "added_at": "2026-02-26T10:00:00+09:00",
        "cached_price": 72500.0,
        "cached_at": "2026-02-26T14:30:00+09:00"
      }
    ]
  }
}
```
- 최상위 키: Telegram user_id (문자열)
- market: `KRX` | `NAS` | `NYS` | `AMS` | `BOND`
- `name`: 시세 조회 시 API에서 자동 캐싱. `/port add` 시 직접 입력 가능.
- `cached_price` / `cached_at`: 마지막 성공 조회 가격. 조회 실패 시 폴백용. `⏱` 마커로 표시.
- **BOND 전용**: `quantity` = 액면가 1,000원 단위, `avg_price`/`cached_price` = 10,000원 액면 기준 가격
  - 평가금액 = `price × quantity × 0.1` (예: qty=50000, price=7485 → 37,425,000원)

### signals.json
```json
{
  "42621862": {
    "signals": [
      {
        "id": "fc8f512e-ca6b-4a86-8ed4-e442863919db",
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
}
```
- 최상위 키: Telegram user_id (문자열)
- `id`: UUID v4 (내부 식별용, 사용자에게 노출 안 됨)
- 사용자에게는 `/signal list`의 순서 번호로 표시 및 삭제

**condition types:**
| type | params | 설명 |
|---|---|---|
| `price_above` | `{ target: f64 }` | 현재가 ≥ target |
| `price_below` | `{ target: f64 }` | 현재가 ≤ target |
| `profit_above` | `{ percentage: f64 }` | 수익률 ≥ % (매입가 대비) |
| `profit_below` | `{ percentage: f64 }` | 수익률 ≤ % |

**시그널 동작**: 1회성 발동 → 텔레그램 전송 성공 시에만 `active: false`. 전송 실패 시 `active` 유지 → 다음 주기 재시도.

---

## 텔레그램 봇 명령어

### 포트폴리오 관리 (`/port`)
| 명령어 | 설명 |
|---|---|
| `/port add [마켓] [종목코드] [수량] [매입가] [@계좌]` | 종목 추가 (종목명 API 자동 조회, 실패 시 거부) |
| `/port remove [종목코드]` | 종목 삭제 |
| `/port edit [종목코드] [수량] [매입가]` | 종목 수정 |
| `/port list [@계좌]` | 전체 포트폴리오 + 현재가 + 손익 (계좌별 필터 가능) |
| `/port info [종목코드]` | 개별 종목 상세 + 시그널 |
| `/port summary` | 자산배분 요약 + 총 손익 |

### 시그널 관리 (`/signal`)
| 명령어 | 설명 |
|---|---|
| `/signal add [종목코드] [> 또는 <] [값 또는 수익률%]` | 시그널 설정 (예: `> 80000`, `> 10%`) |
| `/signal list` | 전체 시그널 조회 (번호 포함) |
| `/signal remove [번호]` | 번호로 삭제. 여러 개: `/signal remove 1 2` |
| `/signal clear [종목코드]` | 종목 시그널 전체 삭제 |

> ⚠️ `/signal remove` 시 목록 확인 후, 여러 개는 한 번에 입력 (번호가 삭제 후 재정렬됨)

### 사용자 관리 (`/user`) — 오너 전용
| 명령어 | 설명 |
|---|---|
| `/user add [chat_id]` | 사용자 추가 |
| `/user remove [chat_id]` | 사용자 삭제 |
| `/user list` | 허용된 사용자 목록 |

- `telegram.owner_chat_id == 0`: 첫 명령 발신자를 오너로 자동 등록 후 config.json 저장
- owner는 항상 허용. 추가 허용 유저 목록은 `config.json`의 `telegram.users`에 저장
- 미허용 유저가 명령 시 `"접근 권한이 없습니다. (chat_id: xxx)"` 응답 → owner가 필요 시 추가 가능
- `owner_chat_id` 캐싱: `thread_local! Cell<Option<i64>>`으로 메모리 유지. 최초 1회만 config.json 읽음. 오너 등록 시 `set_owner_chat_id()` 호출로 즉시 갱신.

### 시스템
| 명령어 | 설명 |
|---|---|
| `/status` | 시스템 상태 |
| `/ping` | 응답 확인 |
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
| 토큰 갱신 | 만료 1시간 전 | `expires_at` 기준 자동 판단 (API Actor가 매 요청 전 체크) |

- 환율 전용 스케줄 없음. 해외주식 시그널 체크(5분) 시 `GetOverseasPrice` t_rate 부산물로 자동 갱신됨.

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
teloxide = { version = "0.13", default-features = false, features = ["macros", "rustls", "ctrlc_handler"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
tracing-appender = "0.2"
anyhow = "1"
uuid = { version = "1", features = ["v4"] }
```

---

## Market 헬퍼 메서드 (src/models/portfolio.rs)

| 메서드 | 역할 |
|---|---|
| `Market::from_str(s)` | 문자열 → Market (대소문자 무관) |
| `Market::is_open_now()` | KST 현재 시각 기준 장중 여부 |
| `Market::exchange_code()` | 해외주식 EXCD 코드 (NAS/NYS/AMS만 반환) |
| `Market::value_factor()` | 평가금액 보정계수. BOND=0.1, 나머지=1.0 |
| `Market::product_type_code()` | 상품기본조회 PRDT_TYPE_CD. KRX=300, NAS=512, NYS=513, AMS=529, BOND=302 |

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
9. **입력 정규화**: 종목코드·마켓코드 대문자 변환은 `cmd_port`/`cmd_signal` 진입점에서 `rest.to_uppercase()` 1회 적용. 개별 서브커맨드에서 중복 변환 금지.
10. **종목명**: `/port add` 시 API 자동 조회 전용 (수동 입력 불가). 조회 실패 = 추가 거부.
11. **비커맨드 메시지**: `Dispatcher::default_handler(|_| async {})` — 슬래시 없는 일반 메시지는 디스패처 레벨에서 무시.

---

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

### /port list
```
📊 포트폴리오 현황
2026-02-26 14:30 기준

🇰🇷 국내
• 005930 삼성전자 | 10 | 70,000→72,500 | +3.6%
• 069500 KODEX200 | 100 | 35,000→34,200 | -2.3%

🇺🇸 미국
• TSLA 테슬라 | 5 | $180.50→$195.20 | +8.1%

🏛 채권
• KR103502G990 국고01125-3909(19-6) | 50,000 | 7,435→7,485 | +0.7%

──────────
💰 총 평가: 145,234,500원
💵 총 손익: +1,234,500원 (+0.9%)
💱 USD/KRW: 1,450
```
- 총 평가 = KRX원화 + 채권원화 + (미국달러 × usd_krw)
- `💱 USD/KRW` 줄: 미국 종목이 있을 때만 표시
- `⏱` 마커: 직전 캐시 가격 사용 (실시간 조회 실패 시)

### /port info [종목코드] (국내)
```
📈 005930 삼성전자
현재가: 72,500원 (전일 대비 +1.2%)
매입가: 70,000원 × 10
평가금액: 725,000원
손익: +25,000원 (+3.6%)

⚡ 설정된 시그널:
• 가격 ≥ 80,000 → 알림
• 수익률 ≥ 20% → 알림
```

### /port info [종목코드] (미국)
```
📈 TSLA 테슬라
현재가: $195.20 (전일 대비 +2.0%)
매입가: $180.50 × 5
평가금액: $976.00 (약 1,415,200원)
손익: +$73.50 (+8.1%)
💱 USD/KRW: 1,450
```

### /port summary
```
📊 포트폴리오 요약
🇰🇷 국내: 69,521,000원 (65%)
🇺🇸 미국: 26,467,000원 (25%)
🏛 채권: 37,425,000원 (35%)
──────────
💰 총 평가: 133,413,000원
💵 총 손익: +3,234,500원 (+2.5%)
💱 USD/KRW: 1,450
```
- `💱 USD/KRW` 줄: 미국 종목 평가금액이 0보다 클 때만 표시
