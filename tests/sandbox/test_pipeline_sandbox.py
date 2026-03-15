#!/usr/bin/env python3
"""
사냥 파이프라인 샌드박스 테스트
Flash Lite 사냥 → (모의 수집) → Gemma 평가

사용법:
  export GEMINI_API_KEY="AIza..."
  python3 test_pipeline_sandbox.py
"""
import json
import os
import sys
import time
import random
import requests

GEMINI_URL = "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={api_key}"
#HUNT_MODEL = "gemini-2.5-flash-lite"
HUNT_MODEL = "gemini-3.1-flash-lite-preview"
JUDGE_MODEL = "gemma-3-27b-it"

HUNT_PROMPT = """\
다음 조건을 만족하는 미국 주식 목록을 만든다
- 가격 < $10
- 처물려도 괜찮을 정도의 뒷배나 숨겨둔 현금이나 매출이 있을 것
- 파리떼가 아직 덜꼬인 신선한 사냥감
- 의외로 인간들이 잘모르는 섹터나 기술
- 현재 상장중

목록의 형태
- 티커, 거래소, 한 줄 설명
- 이외의 내용은 붙이지 않는다
"""

# ── 색상 ──
GREEN = "\033[92m"
RED = "\033[91m"
CYAN = "\033[96m"
RESET = "\033[0m"

def header(step, title):
    print(f"\n{CYAN}{'='*60}")
    print(f"  [{step}] {title}")
    print(f"{'='*60}{RESET}\n")

def fail(msg):
    print(f"{RED}FAIL: {msg}{RESET}")
    sys.exit(1)

def ok(msg):
    print(f"{GREEN}OK: {msg}{RESET}")

# ── 1단계: Flash Lite 사냥 ──
def step_hunt(api_key):
    header("1/3", "Flash Lite 사냥")

    candidate_count = 15
    prompt = f"""{HUNT_PROMPT}
Return exactly {candidate_count} items as a JSON array:
[{{"ticker":"XXX","market":"NAS","name":"Company Name","sector":"Sector","reason":"한 줄 설명"}}]
market: NAS (NASDAQ), NYS (NYSE), AMS (AMEX).
No other text, no markdown.

Exclude these blacklisted tickers: none"""

    text = call_llm(api_key, HUNT_MODEL, prompt)
    candidates = parse_json_array(text)

    print(f"  사냥 결과: {len(candidates)}개")
    for c in candidates:
        print(f"    {c.get('ticker','?'):6s} [{c.get('market','?')}] {c.get('name','?')[:25]:25s} — {c.get('reason','?')[:40]}")

    ok(f"사냥 완료 ({len(candidates)}개)")
    return candidates

# ── 2단계: 모의 수집 ──
def step_collect_mock(candidates):
    header("2/3", "모의 수집 (한투 API 대신 더미 데이터)")

    collected = []
    for c in candidates:
        ticker = c.get("ticker", "???")
        price = round(random.uniform(1.0, 9.99), 2)
        detail = (
            f"Ticker: {ticker}\n"
            f"Name: {c.get('name', ticker)}\n"
            f"Price: ${price:.2f} ({random.uniform(-5, 5):+.2f}%)\n"
            f"Market Cap: {random.randint(50, 5000)}M\n"
            f"PER: {round(random.uniform(-50, 50), 1)}, PBR: {round(random.uniform(0.5, 15), 1)}\n"
            f"EPS: {round(random.uniform(-2, 2), 2)}, BPS: {round(random.uniform(0.1, 10), 2)}\n"
            f"Shares Outstanding: {random.randint(10, 500)}M\n"
            f"Volume: {random.randint(100000, 50000000)} (prev: {random.randint(100000, 50000000)})\n"
            f"52W High: ${round(price * random.uniform(1.2, 3.0), 2):.2f}, "
            f"Low: ${round(price * random.uniform(0.3, 0.8), 2):.2f}\n"
            f"Sector: {c.get('sector', 'Unknown')}\n"
        )
        collected.append({"ticker": ticker, "detail": detail})
        print(f"  {ticker}: ${price:.2f}")

    ok(f"모의 수집 완료 ({len(collected)}개)")
    return collected

# ── 3단계: Gemma 평가 ──
def step_judge(api_key, collected):
    header("3/3", "Gemma 평가")

    combined = "\n---\n".join(c["detail"] for c in collected)

    prompt = f"""You are a financial analyst evaluating US small-cap stocks under $10.
Score each stock 0-100 based on:
- Financial health (PER, PBR, EPS)
- Growth potential
- Risk assessment
- Market position

Here is the real market data for each stock:
{combined}

Return a JSON array with your evaluation:
[{{"ticker":"XXX","score":85,"verdict":"..."}}]
score: 0-100, verdict: brief explanation.
No other text, no markdown."""

    text = call_llm(api_key, JUDGE_MODEL, prompt)
    results = parse_json_array(text)

    print(f"  평가 결과: {len(results)}개")
    min_score = 60.0
    survived = 0
    culled = 0
    for r in sorted(results, key=lambda x: x.get("score", 0), reverse=True):
        ticker = r.get("ticker", "?")
        score = r.get("score", 0)
        verdict = r.get("verdict", "")[:50]
        status = f"{GREEN}생존{RESET}" if score >= min_score else f"{RED}처단{RESET}"
        if score >= min_score:
            survived += 1
        else:
            culled += 1
        print(f"    {ticker:6s} {score:5.1f}점 {status} — {verdict}")

    ok(f"평가 완료 (✅{survived}생존 ⚖️{culled}처단)")
    return results

# ── LLM API 호출 ──
def call_llm(api_key, model, prompt):
    url = GEMINI_URL.format(model=model, api_key=api_key)
    body = {"contents": [{"parts": [{"text": prompt}]}]}

    print(f"  LLM 호출 중 ({model})...", end=" ", flush=True)
    t0 = time.time()
    resp = requests.post(url, json=body, headers={"Content-Type": "application/json"}, timeout=60)
    elapsed = time.time() - t0
    print(f"({elapsed:.1f}s)")

    if resp.status_code != 200:
        fail(f"LLM API {resp.status_code}: {resp.text[:200]}")

    data = resp.json()

    if "error" in data:
        fail(f"LLM error: {data['error'].get('message', 'unknown')}")

    text = data.get("candidates", [{}])[0].get("content", {}).get("parts", [{}])[0].get("text", "")
    if not text:
        fail("LLM 응답이 비어있습니다")

    return text

# ── JSON 배열 파싱 ──
def parse_json_array(text):
    cleaned = text.strip()
    if cleaned.startswith("```json"):
        cleaned = cleaned[7:]
    elif cleaned.startswith("```"):
        cleaned = cleaned[3:]
    if cleaned.endswith("```"):
        cleaned = cleaned[:-3]
    cleaned = cleaned.strip()

    start = cleaned.find("[")
    end = cleaned.rfind("]")
    if start == -1 or end == -1:
        fail(f"JSON 배열 없음: {text[:100]}")

    try:
        return json.loads(cleaned[start:end+1])
    except json.JSONDecodeError as e:
        fail(f"JSON 파싱 실패: {e}\n원문: {cleaned[start:start+200]}")

# ── main ──
def main():
    gemini_key = os.environ.get("GEMINI_API_KEY", "")

    if not gemini_key:
        fail("GEMINI_API_KEY 환경변수 설정 필요")

    print(f"{CYAN}{'='*60}")
    print(f"  사냥 파이프라인 샌드박스 테스트")
    print(f"  Flash Lite 사냥 → 모의 수집 → Gemma 평가")
    print(f"{'='*60}{RESET}")

    # 1. Flash Lite 사냥
    candidates = step_hunt(gemini_key)
    if not candidates:
        fail("후보가 0개입니다")

    # 2. 모의 수집
    collected = step_collect_mock(candidates)

    # 3. Gemma 평가
    results = step_judge(gemini_key, collected)

    # 결과 요약
    print(f"\n{CYAN}{'='*60}")
    print(f"  파이프라인 테스트 완료!")
    print(f"  사냥 {len(candidates)}개 → 수집 {len(collected)}개 → 평가 {len(results)}개")
    print(f"{'='*60}{RESET}")

if __name__ == "__main__":
    main()
