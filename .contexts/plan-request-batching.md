# API 요청 배치 처리 계획

## 배경
- 현재: 요청 1개당 rate_limit(50ms) → API call → 응답 대기 → 다음 요청 (완전 직렬)
- 30종목 = 30 × (50ms + ~800ms) = ~25초
- API 응답 대기 시간 동안 Actor가 놀고 있음

## 설계 원칙
- Mutex/RwLock 금지 (CLAUDE.md)
- Actor + 채널 유지
- current_thread 단일 스레드 유지

## 전략 A: 배치 수확 (단순)

현재 구조에서 최소 변경. 배치로 모아서 순차 처리.

```rust
loop {
    let batch = collect_batch(&mut rx, 5).await;
    for req in batch {
        ctx.rate_limit().await;
        let result = call_api(&ctx, &req).await;
        let _ = req.respond_to.send(result);
    }
}
```

- rate limit 유지, API 호출은 여전히 순차
- 배치 수확으로 recv 오버헤드만 줄임
- **효과 미미**: 병목이 API 왕복 시간이라 배치로 모아도 순차면 같음

## 전략 B: 파이프라이닝 (FuturesUnordered)

Actor 내에서 발신만 rate limit 지키고, 응답은 비동기 수거.

```
직렬:      [발신+대기800ms][발신+대기800ms][발신+대기800ms]...  = 25초
파이프라인: [발신][발신][발신]...[발신]                          = 1.5초 발신
                 [────응답수거────]                              + 0.8초
                                                        합계 = 2.3초
```

### 구현 스케치

```rust
use futures::stream::FuturesUnordered;
use futures::StreamExt;

let mut in_flight = FuturesUnordered::new();

loop {
    tokio::select! {
        // 새 요청 수신 → rate limit 후 발사 (응답 안 기다림)
        Some(req) = rx.recv() => {
            ctx.rate_limit().await;
            let headers = ctx.common_headers(tr_id)?;
            let client = ctx.client.clone();  // cheap: 커넥션 풀 공유

            in_flight.push(async move {
                let resp = client.get(url).headers(headers).send().await;
                (respond_to, resp)
            });
        }
        // 날아온 응답 수거
        Some((respond_to, resp)) = in_flight.next() => {
            let parsed = parse_response(resp);
            // usd_krw 갱신 등 부수효과 처리
            let _ = respond_to.send(parsed);
        }
    }
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

## 판단

| 전략 | 효과 | 복잡도 | 추천 |
|------|------|--------|------|
| A (배치 수확) | 미미 | 낮음 | ❌ |
| B (파이프라인) | 높음 (API 느릴수록) | 중간 | ✅ |

## threshold
- 현재 30종목: 캐시 전략(plan-cache-strategy.md)으로 충분
- 100종목+ 또는 한투 API 지연 반복 시 전략 B 도입 검토

## 확인 파일
- `src/api/actor.rs`: `run_api_actor` 함수
