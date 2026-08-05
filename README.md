# CLAUDE.md — gidbcn (Finance Signal Alert)

## 프로젝트 개요
금융 시그널 포착 및 텔레그램 알림 시스템.
텔레그램 봇 명령어로 포트폴리오 관리, 한투 Open API 시세 조회, 시그널 조건 충족 시 알림.

**핵심 제약**: 한투 계좌 연동 없음. 시세 데이터 조회 용도로만 사용.
**배포**: OCI 인스턴스, `make deploy-oci` (stop → scp → start)

---

## 아키텍처 요약

- **런타임**: tokio `current_thread` + `LocalSet` (싱글스레드, 뮤텍스 프리)
- **Actor 2개**: Bot Task (메인) + API Actor (한투 API 전담)
- **통신**: mpsc + oneshot 채널. Mutex/RwLock/Arc<Mutex<T>> 사용 금지.
- **저장소**: SQLite WAL (`thread_local! RefCell<Connection>`)
- **파일 I/O**: `std::fs` (수 KB, `tokio::fs`/`spawn_blocking` 미사용)

```
┌──────────────────────────────────────────────────────┐
│  tokio current_thread + LocalSet                      │
│                                                       │
│  ┌────────────────────┐  mpsc   ┌──────────────────┐ │
│  │    Bot Task         │◄──────►│   API Actor        │ │
│  │  • teloxide 디스패처 │ oneshot│  • reqwest         │ │
│  │  • 명령어 처리       │        │  • 토큰 관리       │ │
│  │  • 시그널 엔진       │        │  • rate limit      │ │
│  │  • 스케줄러          │        └──────────────────┘ │
│  │  • SQLite I/O       │                              │
│  └────────────────────┘                               │
└──────────────────────────────────────────────────────┘
```

상세: [docs/architecture.md](docs/architecture.md)

---

## 프로젝트 구조

```
src/
├── main.rs            ← 런타임 생성 → API Actor spawn → Bot 실행
├── config.rs          ← Config 구조체, JSON 로드/저장
├── storage.rs         ← Config 싱글턴 + SQLite CRUD
├── scheduler.rs       ← tokio::time::interval 기반 select! 루프
├── api/
│   ├── actor.rs       ← ApiActor 수신 루프 + ApiHandle
│   ├── auth.rs        ← OAuth 토큰 발급/갱신
│   ├── domestic.rs    ← 국내주식 현재가
│   ├── overseas.rs    ← 해외주식 현재가 (환율 부산물)
│   ├── bond.rs        ← 국내채권 현재가
│   └── stock_info.rs  ← 종목명 조회 (CTPF1604R)
├── bot/
│   ├── handler.rs     ← teloxide 디스패처
│   ├── commands.rs    ← /port, /signal, /w 서브커맨드
│   └── formatter.rs   ← 텔레그램 메시지 포맷
├── signal/
│   ├── engine.rs      ← 시그널 조건 평가
│   └── price.rs       ← price_above/below, profit_above/below
├── watchlist/
│   ├── pipeline.rs    ← 사냥(run_hunt) / 가죽 작업(run_evaluate) 독립 사이클
│   ├── gemini.rs      ← Google AI Studio API 호출
│   ├── db.rs          ← 워치리스트 SQLite CRUD
│   └── models.rs      ← Candidate, HuntResult, JudgeResult
└── models/
    ├── portfolio.rs   ← Holding, Market enum
    ├── signal.rs      ← Signal, Condition
    └── messages.rs    ← ApiRequest enum
```

**데이터 경로** (레포 외부): `/opt/kkuepark/gidbcn/`
- `config.json` — API 키, 토큰, 허용 사용자 목록
- `portfolio.db` — SQLite WAL: holdings, signals, candidates, blacklist, prompts 등

---

## 한투 Open API

### 공통
- Base URL: `https://openapi.koreainvestment.com:9443`
- 인증: Bearer token + appkey + appsecret 헤더
- **초당 20회 제한**, 토큰 발급 **1분당 1회** (유효 24시간)
- 모든 숫자를 String으로 반환 → 파싱 필요

### 공통 헤더
```
Content-Type: application/json; charset=utf-8
authorization: Bearer {access_token}
appkey: {APP_KEY}
appsecret: {APP_SECRET}
tr_id: {거래ID}
custtype: P
```

### 엔드포인트

| # | 용도 | 메서드 | 경로 | tr_id | 핵심 필드 |
|---|------|--------|------|-------|-----------|
| 0 | 토큰 발급 | POST | `/oauth2/tokenP` | - | `access_token`, `access_token_token_expired` (KST) |
| 1 | 국내주식 | GET | `/uapi/domestic-stock/v1/quotations/inquire-price` | `FHKST01010100` | `stck_prpr`(현재가), `prdy_ctrt`(등락률) |
| 2 | 해외주식 | GET | `/uapi/overseas-price/v1/quotations/price-detail` | `HHDFS76200200` | `last`, `t_xrat`(등락률), `t_rate`(환율) |
| 3 | 국내채권 | GET | `/uapi/domestic-bond/v1/quotations/inquire-price` | `FHKBJ773400C0` | `bond_prpr`, `prdy_ctrt` |
| 4 | 종목명 | GET | `/uapi/domestic-stock/v1/quotations/search-info` | `CTPF1604R` | `prdt_abrv_name` |

**주요 주의사항**:
- 토큰 만료: `access_token_token_expired` 파싱 (KST). 파싱 실패 시 +24h fallback
- 해외주식 `t_rate` → actor 메모리 환율 갱신 (파일 저장 없음, `GetExchangeRate` → 메모리 반환)
- 채권 `bond_prpr` = 액면 10,000원 기준 가격. 평가금액 = `price × qty × 0.1`
- 종목명 `Market::product_type_code()`: KRX=300, NAS=512, NYS=513, AMS=529, BOND=302

### 마켓 코드

| 마켓 | 한투 API | 비고 |
|------|----------|------|
| `KRX` | `FID_COND_MRKT_DIV_CODE=J` | 주식/ETF |
| `NAS`/`NYS`/`AMS` | `EXCD={코드}` | 해외주식 |
| `BOND` | `FID_COND_MRKT_DIV_CODE=B` | 채권 별도 엔드포인트 |
| `CART` | - | 수동 관리 (API 조회 없음, `cached_price` 직접 사용) |

---

## DB 스키마 (SQLite, portfolio.db)

### holdings
```sql
CREATE TABLE holdings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL, market TEXT NOT NULL,
    symbol TEXT NOT NULL, name TEXT NOT NULL DEFAULT '',
    account TEXT NOT NULL DEFAULT '',
    quantity REAL NOT NULL, avg_price REAL NOT NULL,
    added_at TEXT NOT NULL,
    cached_price REAL, cached_at TEXT,
    UNIQUE(user_id, symbol, account)
);
```

### signals
```sql
CREATE TABLE signals (
    id TEXT PRIMARY KEY,  -- UUID v4
    user_id INTEGER NOT NULL, symbol TEXT NOT NULL,
    account TEXT NOT NULL DEFAULT '',
    cond_type TEXT NOT NULL,  -- price_above/below, profit_above/below
    cond_value REAL NOT NULL, active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL
);
```

### pending (사냥 버퍼)
```sql
CREATE TABLE pending (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ticker TEXT NOT NULL UNIQUE, market TEXT, name TEXT,
    sector TEXT, reason TEXT, hunt_score REAL,
    hunt_count INTEGER NOT NULL DEFAULT 1,
    strike_count INTEGER NOT NULL DEFAULT 0,  -- 한투 API 수집 실패 누적. 3회면 BL
    prompt_id INTEGER, created_at TEXT NOT NULL
);
```

### candidates (감정 완료)
```sql
CREATE TABLE candidates (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ticker TEXT NOT NULL UNIQUE, market TEXT, name TEXT,
    sector TEXT, reason TEXT, hunt_score REAL,
    hunt_count INTEGER NOT NULL DEFAULT 1,
    score REAL, verdict TEXT, detail_text TEXT DEFAULT '',
    status TEXT NOT NULL DEFAULT 'judged',  -- judged/blacklisted
    strike_count INTEGER NOT NULL DEFAULT 0,
    judged_at TEXT, created_at TEXT NOT NULL
);
```

---

## config.json 스키마

```json
{
  "kis_api": { "app_key": "", "app_secret": "", "base_url": "https://openapi.koreainvestment.com:9443", "hts_id": "", "token": { "access_token": "", "expires_at": "" } },
  "telegram": { "bot_token": "", "owner_chat_id": 0, "users": [] },
  "scheduler": { "signal_check_interval_minutes": 5 },
  "log": { "dir": "/opt/kkuepark/gidbcn", "retain_days": 7 },
  "watchlist": { ... }
}
```

### watchlist 설정
| 키 | 기본값 | 설명 |
|---|---|---|
| `hunt_models` | ["gemini-3.1-flash-lite", "gemini-2.5-flash-lite", "gemini-2.5-flash"] | 사냥 모델 (폴백 순서). Gemma는 mimeType=JSON 미지원으로 제외 |
| `judge_models` | ["gemini-3.1-flash-lite", "gemini-2.5-flash-lite"] | 평가 모델 (폴백 순서) |
| `max_hunt_calls_per_day` | 50 | 일일 사냥 호출 한도 |
| `max_judge_calls_per_day` | 50 | 일일 평가 호출 한도 |
| `max_survivors` | 50 | 도태 후 생존 상한 |
| `hunt_count_weight` | 3.0 | 반복 추천 보너스 계수 (도태 판정용) |
| `candidate_count` | 30 | 사냥당 후보 수 |
| `hunt_interval_minutes` | 30 | 사냥 주기 (분) |
| `min_score` | 60.0 | 척살 기준 점수 |
| `retention_days` | 100 | 데이터 보관 기간 (일) |

---

## 텔레그램 명령어

### /port (포트폴리오)
| 명령어 | 설명 |
|---|---|
| `/port add [마켓] [종목코드] [수량] [매입가] [@계좌]` | 종목 추가 (종목명 API 자동 조회, 실패 시 거부) |
| `/port add CART [이름] [수량] [매입가] [@계좌] [=현재가]` | CART 수동 추가 |
| `/port remove [종목코드]` | 삭제 |
| `/port edit [종목코드] [수량] [매입가]` | 수정 |
| `/port list [@계좌]` | 현황 (현재가 + 손익) |
| `/port info [종목코드]` | 개별 상세 |
| `/port summary` | 자산배분 요약 |
| `/port export [@계좌]` | CSV 파일 전송 |

### /signal (시그널)
| 명령어 | 설명 |
|---|---|
| `/signal add [종목코드] [>/< 값 또는 %]` | 시그널 설정 |
| `/signal list` | 전체 조회 |
| `/signal remove [번호]` | 삭제 (여러 개 가능) |
| `/signal clear [종목코드]` | 종목 시그널 전체 삭제 |

### /w (워치리스트)
| 명령어 | 설명 |
|---|---|
| `/w hunt` | 사냥 시작 (즉시 1회 + 자동 주기) |
| `/w stop` | 사냥 중지 |
| `/w eval` | 수동 가죽 작업 1회 (자동 사이클 외 즉시 트리거) |
| `/w ls` | 포획 게코 (기본) |
| `/w ls pelt` | 가죽 현황 (점수순) |
| `/w info [TICKER]` | 종목 상세 |
| `/w bl` / `/w bl add` / `/w bl rm` | 블랙리스트 관리 (수동 BL = 영구) |
| `/w budget` | API 사용량 |
| `/w prompt hunt\|judge show\|set` | 프롬프트 관리 |
| `/w hist` | Gemini 호출 이력 |
| `/w clear gecko\|pelt\|bl` | 일괄 삭제 |

### /user (오너 전용)
| 명령어 | 설명 |
|---|---|
| `/user add\|remove\|list` | 허용 사용자 관리 |

### 시스템
`/status`, `/ping`, `/help`

---

## 스케줄러

| 작업 | 주기 | 조건 |
|------|------|------|
| 시그널 체크 | 5분 | 장중 (KRX 09:00~15:30, US 22:30~05:00 KST) |
| 사냥 사이클 | 30분 | discovery_enabled && prompts OK && hunt 한도 미초과 |
| 가죽 작업 | KST 02:00, 14:00 하루 2회 | discovery_enabled && judge 한도 미초과 |
| 토큰 갱신 | 매 요청 전 체크 | 만료 1시간 전 자동 |

---

## Gemini API 한도

- **일일 리셋**: 태평양시간 자정 (고정 UTC-8, Google AI Studio 동기화)
- **429 PerMinute**: `retryDelay + 5초` 대기 → 같은 모델 1회 재시도
- **429 RPD 등**: 즉시 다음 모델 폴백
- **모델 폴백**: config 배열 순회 (당일 성공 모델 우선)
- **가죽 작업 배치**: `candidate_count` 단위 분할, 배치 간 60초 대기

---

## 코딩 규칙

1. **Mutex/RwLock/Arc<Mutex<T>> 절대 금지**. Actor + 채널로만 통신.
2. **`tokio::fs` 금지**. `std::fs` 동기 I/O.
3. **`tokio-cron-scheduler` 금지**. `tokio::time::interval` 직접 구현.
4. **에러**: `anyhow`. `unwrap()` 최소화.
5. **로깅**: `tracing` (`info!`, `warn!`, `error!`). 파일 로그(`tracing-appender`) guard는 `std::mem::forget`으로 유지 — drop 시 writer 스레드가 종료되어 파일에 기록되지 않음.
6. **TLS**: rustls. OpenSSL 금지.
7. **rate limiting**: API Actor 내부 `tokio::time::sleep` (초당 20회).
8. **입력 정규화**: 진입점에서 `rest.to_uppercase()` 1회. 개별 서브커맨드에서 중복 금지.
9. **종목명**: `/port add` 시 API 자동 조회 전용. 실패 = 추가 거부.
10. **비커맨드**: `default_handler(|_| async {})` — 무시.
11. **CART**: 수동 관리. API 조회 없음. 시그널 불허.
