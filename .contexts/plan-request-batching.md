# API 요청 배치 처리 계획

## 배경
- 현재: 요청 1개당 rate_limit(50ms) 순차 처리
- 30종목 = 1.5초 (rate limit만) + network latency
- `/port list`, signal check 시 직렬로 인해 응답 지연

## 설계 원칙
- Mutex/RwLock 금지 (CLAUDE.md)
- Actor + 채널 유지
- current_thread 단일 스레드 유지

## 변경 안 (actor.rs)

### 현재 구조
```
rx.recv() → rate_limit(50ms) → API call → response → repeat
```

### 수정 후 구조
```rust
// 버퍼된 요청을 배치로 처리
loop {
    // 요청을 버퍼에서 수집 (non-blocking recv)
    let batch = collect_batch(&mut rx, 5).await;
    
    for req in batch {
        ctx.rate_limit().await;  // 50ms 간격 유지
        let result = call_api(&ctx, &req).await;
        let _ = req.respond_to.send(result);
    }
}

async fn collect_batch(rx: &mut Receiver<ApiRequest>, max: usize) -> Vec<ApiRequest> {
    let mut batch = vec![];
    while let Some(req) = rx.recv().now_or_await().ok() {
        batch.push(req);
        if batch.len() >= max {
            break;
        }
    }
    batch
}
```

## 기준
| 항목 | 기존 | 수정 후 |
|------|------|--------|
| rate limit | 요청당 50ms | 동일 |
| API 호출 | 순차 | 순차 (동일) |
| 차이 | - | 배치 수확-delay 감소 |

## threshold
- 관리 종목 100개+ 또는 실시간 알림 필요 시 도입
- 현재 30개: 우선순위 낮음

## 확인 파일
- `src/api/actor.rs`: `run_api_actor` 함수 (~108-166 line)