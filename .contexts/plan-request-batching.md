# API 요청 파이프라이닝 계획

## 배경
- 현재: 요청 1개당 rate_limit(50ms) → API call → 응답 대기 → 다음 요청 (완전 직렬)
- 30종목 = 30 × (50ms + ~800ms) = ~25초
- API 응답 대기 시간 동안 Actor가 놀고 있음

## 설계 원칙
- Mutex/RwLock 금지
- Actor + 채널 유지
- current_thread 단일 스레드 유지

## 전략 B: 파이프라이닝 (FuturesUnordered) — 추천

Actor 내에서 발신만 rate limit 지키고, 응답은 비동기 수거.

```
직렬:      [발신+대기800ms][발신+대기800ms][발신+대기800ms]...  = 25초
파이프라인: [발신][발신][발신]...[발신]                          = 1.5초 발신
                 [────응답수거────]                              + 0.8초
                                                        합계 = 2.3초
```

### 핵심 문제: `&ActorContext` 수명

현재 API 함수들이 `&ActorContext`를 빌림:
```rust
domestic::get_price(ctx: &ActorContext, symbol) -> Result<PriceData>
```
future를 FuturesUnordered에 넣으려면 `'static`이 필요 → `&ctx` 빌림 불가.

### 해법: API 함수 시그니처 변경

API 함수가 `&ActorContext`에서 쓰는 건 3가지뿐:
1. `ctx.config.base_url` — URL 빌드
2. `ctx.common_headers(tr_id)` — 인증 헤더
3. `ctx.send_with_retry(ctx.client.get(...))` — HTTP 전송

전부 **발신 전에 준비 가능**. Actor 루프에서 미리 빌드해서 넘기면 됨.

```rust
// 현재
domestic::get_price(ctx: &ActorContext, symbol: &str) -> Result<PriceData>

// 변경
domestic::get_price(client: &reqwest::Client, url: &str, headers: HeaderMap, symbol: &str) -> Result<PriceData>
```

`send_with_retry`도 ActorContext 메서드 → free function으로 분리.

변경 대상: domestic.rs, overseas.rs, bond.rs, stock_info.rs (각 1~2개 함수).
**실질적 변경량 적음** — 시그니처 + ctx.xxx 호출부 치환.

### 구현 스케치

```rust
use futures::stream::FuturesUnordered;
use futures::StreamExt;

let mut in_flight = FuturesUnordered::new();

loop {
    tokio::select! {
        Some(req) = rx.recv() => {
            ctx.rate_limit().await;

            match req {
                ApiRequest::GetDomesticPrice { symbol, respond_to } => {
                    let client = ctx.client.clone();
                    let url = format!("{}/uapi/domestic-stock/...", ctx.config.base_url);
                    let headers = ctx.common_headers("FHKST01010100")?;
                    in_flight.push(async move {
                        let result = domestic::get_price(&client, &url, headers, &symbol).await;
                        let _ = respond_to.send(result);
                    });
                }
                // ... 다른 variant도 동일 패턴
            }
        }
        Some(()) = in_flight.next() => {
            // 완료 처리 (usd_krw 갱신 등은 별도 채널로)
        }
    }
}
```

### usd_krw 갱신 문제

해외주식 응답에서 t_rate(환율)를 추출해야 하는데, future 안에서 Actor의 usd_krw를 갱신할 수 없음.

해법: future가 `(respond_to, result, Option<f64>)` 반환 → completion 핸들러에서 갱신.

```rust
Some((respond_to, result, rate)) = in_flight.next() => {
    if let Some(r) = rate { usd_krw = r; }
    let _ = respond_to.send(result);
}
```

### Actor 불변성 유지

| 불변성 | 유지 방법 |
|--------|----------|
| rate limit (50ms) | recv 후 발신 전에 await |
| 토큰 단일 소유 | 발신 전에 headers 빌드 (읽기만) |
| 토큰 갱신 | recv 시점에 체크 |
| usd_krw 갱신 | completion 핸들러에서 |
| Mutex 없음 | reqwest::Client Clone만 (Arc<Pool> 공유) |
| current_thread | spawn 안 함, select!만 |

### 성능 비교

| API 응답 | 직렬 (30종목) | 파이프라인 | 배율 |
|----------|-------------|-----------|------|
| 100ms | 4.5초 | 2.6초 | 1.7× |
| 500ms | 16.5초 | 2.0초 | 8× |
| 800ms | 25.5초 | 2.3초 | 11× |
| 2초 | 61.5초 | 3.5초 | 18× |

### 기각된 대안

**호출 측 join_all**: Actor가 직렬 루프라 mpsc에 미리 쌓아도 하나씩 처리 → 효과 없음.
**Rc<RefCell<ActorContext>>**: rate_limit이 &mut self라 borrow_mut 충돌 위험.
**배치 수확**: 배치로 모아도 순차 처리면 병목 동일 → 효과 미미.

## threshold
- 현재 30종목: 캐시 전략(plan-cache-strategy.md)으로 충분
- 100종목+ 또는 한투 API 지연 반복 시 도입 검토

## 구현 순서

| 단계 | 작업 | 파일 |
|------|------|------|
| 1 | `send_with_retry` → free function 분리 | actor.rs |
| 2 | API 함수 시그니처 변경 (ctx → client, url, headers) | domestic.rs, overseas.rs, bond.rs, stock_info.rs |
| 3 | Actor 루프: FuturesUnordered + select! | actor.rs |
| 4 | usd_krw 갱신 경로 변경 | actor.rs |
| 5 | 테스트 | 기존 테스트 통과 확인 |
