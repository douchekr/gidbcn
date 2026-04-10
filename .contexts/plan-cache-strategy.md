# 캐시 전략 개선 계획

## 배경
- `/port list`에서 cached_price가 있어도 age 확인 없이 매번 API 호출
- 캐시 10분 전也好 그대로 사용 (不合理)
- signal check는 실시간성 우선으로 기존대로 유지

## 변경 내용 (commands.rs)

### 기존 플로우
```
API 호출 → 실패시 cached 있으면 사용
```
- cached age 무관通过

### 수정 후 플로우
```
cached_price + cached_at 있는가?
  └→ Yes: age = now - cached_at
          if age ≤ 1분 → cached 사용
          else        → API 호출 + 실패시 cached 사용
  └→ No : API 호출
```

## 기준
| 조건 | 행동 |
|------|------|
| 캐시 있음 + age ≤ 1분 | cached 사용 |
| 캐시 있음 + 1분 < age ≤ 5분 | API 시도, 실패시 cached 사용 |
| 캐시 없음 | API 호출 |

## 확인 파일
- `src/bot/commands.rs`: `show_portfolio_list` 함수 (~580-670 line)

## 테스트
- 기존 테스트 통과 확인
- 수동 검증: cached price 상태에서 `/port list` 호출 후 로그 확인