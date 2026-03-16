use std::cell::RefCell;

use anyhow::{bail, Context, Result};
use chrono::Datelike;
use serde_json::Value;

use super::db;
use super::models::{HuntResult, JudgeResult, PromptType};
use crate::storage;

// 당일 성공 모델 캐시: (ordinal day, 모델명)
thread_local! {
    static MODEL_CACHE: RefCell<[Option<(u32, String)>; 2]> = RefCell::new([None, None]);
}

const CACHE_HUNT: usize = 0;
const CACHE_JUDGE: usize = 1;

/// 당일 성공 모델이 있으면 리스트 맨 앞으로 올림
fn prioritize_models(models: &[String], cache_idx: usize) -> Vec<String> {
    let today = chrono::Local::now().ordinal();
    MODEL_CACHE.with(|c| {
        let cache = c.borrow();
        if let Some((day, ref model)) = cache[cache_idx] {
            if day == today && models.contains(model) {
                let mut result = vec![model.clone()];
                result.extend(models.iter().filter(|m| *m != model).cloned());
                return result;
            }
        }
        models.to_vec()
    })
}

fn remember_model(cache_idx: usize, model: &str) {
    let today = chrono::Local::now().ordinal();
    MODEL_CACHE.with(|c| {
        c.borrow_mut()[cache_idx] = Some((today, model.to_string()));
    });
}

/// Google AI Studio LLM 단일 모델 호출
async fn call_llm(client: &reqwest::Client, model: &str, prompt: &str) -> Result<String> {
    let api_key = storage::with_config(|c| c.secrets.gemini_api_key.clone());

    if api_key.is_empty() {
        bail!("secrets.gemini_api_key가 설정되지 않았습니다");
    }

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={api_key}"
    );

    let body = serde_json::json!({
        "contents": [{"parts": [{"text": prompt}]}]
    });

    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .context("LLM API 요청 실패")?;

    let status = resp.status();
    let text = resp.text().await.context("LLM 응답 읽기 실패")?;

    if !status.is_success() {
        bail!("LLM API 오류 ({status}): {text}");
    }

    extract_gemini_text(&text)
}

/// 모델 리스트 순회 폴백 — 성공하면 해당 모델 기억, 전부 실패 시 마지막 에러 반환
async fn call_llm_with_fallback(
    client: &reqwest::Client,
    models: &[String],
    prompt: &str,
    cache_idx: usize,
) -> Result<(String, String)> {
    let ordered = prioritize_models(models, cache_idx);
    let mut last_err = None;

    for model in &ordered {
        match call_llm(client, model, prompt).await {
            Ok(text) => {
                remember_model(cache_idx, model);
                return Ok((text, model.clone()));
            }
            Err(e) => {
                tracing::warn!("모델 {model} 실패: {e:#}, 다음 모델 시도");
                last_err = Some(e);
            }
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("모델 리스트가 비어있습니다")))
}

/// Gemini API 응답 JSON에서 텍스트 추출
fn extract_gemini_text(response_body: &str) -> Result<String> {
    let json: Value = serde_json::from_str(response_body)
        .context("Gemini 응답 JSON 파싱 실패")?;

    // 에러 응답 체크
    if let Some(error) = json.get("error") {
        let msg = error["message"].as_str().unwrap_or("unknown error");
        let code = error["code"].as_i64().unwrap_or(0);
        bail!("Gemini API 에러 (code {code}): {msg}");
    }

    let content = json["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .unwrap_or("");

    if content.is_empty() {
        // safety 필터 등으로 빈 응답인 경우
        let finish_reason = json["candidates"][0]["finishReason"]
            .as_str()
            .unwrap_or("UNKNOWN");
        if finish_reason == "SAFETY" {
            bail!("Gemini 안전 필터에 의해 응답이 차단되었습니다");
        }
        bail!("Gemini 응답이 비어있습니다 (finishReason: {finish_reason})");
    }

    Ok(content.to_string())
}

/// Gemini 응답에서 JSON 배열 추출 (markdown fence 제거)
fn extract_json_array(text: &str) -> Result<Value> {
    // markdown fence 제거: ```json ... ``` 또는 ``` ... ```
    let cleaned = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    // 첫 번째 [ 부터 마지막 ] 까지 추출
    let start = cleaned.find('[').context("JSON 배열 시작 '[' 없음")?;
    let end = cleaned.rfind(']').context("JSON 배열 끝 ']' 없음")?;
    let array_str = &cleaned[start..=end];

    serde_json::from_str(array_str).context("JSON 배열 파싱 실패")
}

/// 사냥: Flash Lite로 직접 후보 종목 수집
pub async fn hunt(client: &reqwest::Client) -> Result<Vec<HuntResult>> {
    let hunt_prompt = db::get_prompt(PromptType::Hunt)?
        .context("사냥 프롬프트가 설정되지 않았습니다. /w prompt hunt set 으로 설정하세요")?;

    let (hunt_models, candidate_count) = storage::with_config(|c| {
        (c.watchlist.hunt_models.clone(), c.watchlist.candidate_count)
    });

    // 일일 사냥 호출 제한 체크
    let max_calls = storage::with_config(|c| c.watchlist.max_hunt_calls_per_day);
    let today_calls = db::hunt_calls_today()?;
    if today_calls >= max_calls {
        bail!("오늘 사냥 호출 한도 초과 ({today_calls}/{max_calls})");
    }

    let full_prompt = format!(
        "## Instructions\n\
         {hunt_prompt}\n\n\
         ## Output Format\n\
         Return exactly {candidate_count} items as a JSON array:\n\
         ```json\n\
         [{{\"ticker\":\"XXX\",\"market\":\"NAS\",\"name\":\"...\",\"sector\":\"...\",\"reason\":\"...\"}}]\n\
         ```\n\
         - market: NAS (NASDAQ), NYS (NYSE), AMS (AMEX)"
    );

    let response = call_llm_with_fallback(client, &hunt_models, &full_prompt, CACHE_HUNT).await;

    // API 사용 로그
    let _ = db::log_api_call("gemini", "hunt", response.is_ok());

    let (response_text, hunt_model) = response?;

    // JSON 파싱
    let parsed = extract_json_array(&response_text);
    let tickers_str: String;
    let results: Vec<HuntResult>;

    match parsed {
        Ok(arr) => {
            results = serde_json::from_value(arr).unwrap_or_default();
            tickers_str = results.iter().map(|r| r.ticker.as_str()).collect::<Vec<_>>().join(",");
        }
        Err(e) => {
            let _ = db::insert_prompt_history(
                PromptType::Hunt, &full_prompt, &response_text, &hunt_model, "", "parse_error",
            );
            bail!("사냥 결과 파싱 실패: {e}");
        }
    }

    // 이력 저장
    let prompt_id = db::insert_prompt_history(
        PromptType::Hunt, &full_prompt, &response_text, &hunt_model, &tickers_str, "success",
    )?;

    // 블랙리스트 필터링 + DB 저장
    let mut saved = Vec::new();
    for r in &results {
        let ticker = r.ticker.to_uppercase();
        if db::is_blacklisted(&ticker)? {
            tracing::info!("사냥 결과 블랙리스트 제외: {ticker}");
            continue;
        }
        let _ = db::insert_candidate(&ticker, &r.market, &r.name, &r.sector, &r.reason, Some(prompt_id));
        saved.push(r.clone());
    }

    tracing::info!("사냥 완료: {}개 후보 저장 ({}개 블랙리스트 제외)",
        saved.len(), results.len() - saved.len());

    Ok(saved)
}

/// 평가: 한투 데이터 기반으로 Gemma에게 평가 요청 (기준 미달 → 처단)
pub async fn judge(
    client: &reqwest::Client,
    data_text: &str,
) -> Result<Vec<JudgeResult>> {
    // 프롬프트 확인
    let judge_prompt = db::get_prompt(PromptType::Judge)?
        .context("평가(judge) 프롬프트가 설정되지 않았습니다. /w prompt judge set 으로 설정하세요")?;

    let judge_models = storage::with_config(|c| c.watchlist.judge_models.clone());

    // 일일 평가 호출 제한 체크
    let max_calls = storage::with_config(|c| c.watchlist.max_judge_calls_per_day);
    let today_calls = db::judge_calls_today()?;
    if today_calls >= max_calls {
        bail!("오늘 평가 호출 한도 초과 ({today_calls}/{max_calls})");
    }

    let full_prompt = format!(
        "## Instructions\n\
         {judge_prompt}\n\n\
         ## Market Data\n\
         {data_text}\n\n\
         ## Output Format\n\
         Return a JSON array with your evaluation:\n\
         ```json\n\
         [{{\"ticker\":\"XXX\",\"score\":85,\"verdict\":\"...\"}}]\n\
         ```\n\
         - score: 0–100\n\
         - verdict: brief explanation"
    );

    // 평가 모델 호출 (폴백)
    let response = call_llm_with_fallback(client, &judge_models, &full_prompt, CACHE_JUDGE).await;

    let _ = db::log_api_call("gemini", "judge", response.is_ok());

    let (response_text, judge_model) = response?;

    let parsed = extract_json_array(&response_text);
    let tickers_str: String;
    let results: Vec<JudgeResult>;

    match parsed {
        Ok(arr) => {
            results = serde_json::from_value(arr).unwrap_or_default();
            tickers_str = results.iter().map(|r| r.ticker.as_str()).collect::<Vec<_>>().join(",");
        }
        Err(e) => {
            let _ = db::insert_prompt_history(
                PromptType::Judge, &full_prompt, &response_text, &judge_model, "", "parse_error",
            );
            bail!("평가 결과 파싱 실패: {e}");
        }
    }

    let _ = db::insert_prompt_history(
        PromptType::Judge, &full_prompt, &response_text, &judge_model, &tickers_str, "success",
    );

    tracing::info!("평가 완료: {}개 종목", results.len());

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_json_from_clean() {
        let input = r#"[{"ticker":"AAPL","name":"Apple","sector":"Tech","reason":"solid"}]"#;
        let arr = extract_json_array(input).unwrap();
        let results: Vec<HuntResult> = serde_json::from_value(arr).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].ticker, "AAPL");
    }

    #[test]
    fn extract_json_from_markdown_fence() {
        let input = "```json\n[{\"ticker\":\"TSLA\",\"name\":\"Tesla\",\"sector\":\"EV\",\"reason\":\"growth\"}]\n```";
        let arr = extract_json_array(input).unwrap();
        let results: Vec<HuntResult> = serde_json::from_value(arr).unwrap();
        assert_eq!(results[0].ticker, "TSLA");
    }

    #[test]
    fn extract_json_with_preamble() {
        let input = "Here are my picks:\n[{\"ticker\":\"NVDA\",\"score\":92,\"verdict\":\"strong\"}]\nGood luck!";
        let arr = extract_json_array(input).unwrap();
        let results: Vec<JudgeResult> = serde_json::from_value(arr).unwrap();
        assert_eq!(results[0].ticker, "NVDA");
        assert_eq!(results[0].score, 92.0);
    }

    #[test]
    fn extract_json_no_array_fails() {
        let input = "I can't find any stocks right now.";
        assert!(extract_json_array(input).is_err());
    }

    #[test]
    fn extract_multiple_hunt_results() {
        let input = r#"[
            {"ticker":"SOUN","name":"SoundHound AI","sector":"AI","reason":"voice AI platform"},
            {"ticker":"GEVO","name":"Gevo Inc","sector":"Clean Energy","reason":"renewable fuel"},
            {"ticker":"BKSY","name":"BlackSky","sector":"Space","reason":"satellite imagery"}
        ]"#;
        let arr = extract_json_array(input).unwrap();
        let results: Vec<HuntResult> = serde_json::from_value(arr).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].ticker, "SOUN");
        assert_eq!(results[2].sector, "Space");
    }

    #[test]
    fn extract_judge_results_with_scores() {
        let input = r#"```json
        [
            {"ticker":"SOUN","score":78,"verdict":"promising AI play, good revenue growth"},
            {"ticker":"GEVO","score":42,"verdict":"high risk, pre-revenue"},
            {"ticker":"BKSY","score":65,"verdict":"niche market, government contracts"}
        ]
        ```"#;
        let arr = extract_json_array(input).unwrap();
        let results: Vec<JudgeResult> = serde_json::from_value(arr).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].score, 78.0);
        assert_eq!(results[1].score, 42.0);
        assert!(results[1].verdict.contains("pre-revenue"));
    }

    #[test]
    fn hunt_result_missing_optional_fields() {
        // Gemma가 일부 필드를 빠뜨린 경우 default로 처리
        let input = r#"[{"ticker":"XYZ"}]"#;
        let arr = extract_json_array(input).unwrap();
        let results: Vec<HuntResult> = serde_json::from_value(arr).unwrap();
        assert_eq!(results[0].ticker, "XYZ");
        assert_eq!(results[0].name, "");
        assert_eq!(results[0].sector, "");
        assert_eq!(results[0].market, "");
    }

    #[test]
    fn hunt_result_with_market_field() {
        let input = r#"[
            {"ticker":"SOUN","market":"NAS","name":"SoundHound AI","sector":"AI","reason":"voice"},
            {"ticker":"BLNK","market":"NYS","name":"Blink","sector":"EV","reason":"charging"},
            {"ticker":"CBAK","market":"AMS","name":"CBAK Energy","sector":"Battery","reason":"cheap"}
        ]"#;
        let arr = extract_json_array(input).unwrap();
        let results: Vec<HuntResult> = serde_json::from_value(arr).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].market, "NAS");
        assert_eq!(results[1].market, "NYS");
        assert_eq!(results[2].market, "AMS");
    }

    #[test]
    fn extract_json_array_only_brackets() {
        let input = r#"[]"#;
        let arr = extract_json_array(input).unwrap();
        let results: Vec<HuntResult> = serde_json::from_value(arr).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn extract_json_array_broken_json() {
        let input = r#"[{"ticker":"AAA","score":}"#;
        assert!(extract_json_array(input).is_err());
    }

    #[test]
    fn extract_json_array_triple_backtick_no_lang() {
        let input = "```\n[{\"ticker\":\"TEST\",\"score\":77,\"verdict\":\"ok\"}]\n```";
        let arr = extract_json_array(input).unwrap();
        let results: Vec<JudgeResult> = serde_json::from_value(arr).unwrap();
        assert_eq!(results[0].ticker, "TEST");
    }

    #[test]
    fn gemini_response_with_whitespace() {
        let body = r#"{
            "candidates": [{
                "content": {"parts": [{"text": "  \n  hello  \n  "}], "role": "model"},
                "finishReason": "STOP"
            }]
        }"#;
        let text = extract_gemini_text(body).unwrap();
        assert_eq!(text, "  \n  hello  \n  ");
    }

    #[test]
    fn gemini_error_without_message() {
        let body = r#"{"error": {"code": 500}}"#;
        let err = extract_gemini_text(body).unwrap_err();
        assert!(err.to_string().contains("500"));
    }

    #[test]
    fn extract_json_nested_in_text() {
        let input = "Based on your criteria, here are my recommendations:\n\n\
                     [{\"ticker\":\"ABC\",\"score\":88,\"verdict\":\"strong\"}]\n\n\
                     Note: These are not financial advice.";
        let arr = extract_json_array(input).unwrap();
        let results: Vec<JudgeResult> = serde_json::from_value(arr).unwrap();
        assert_eq!(results[0].ticker, "ABC");
        assert_eq!(results[0].score, 88.0);
    }

    // --- Gemini API 응답 파싱 테스트 ---

    #[test]
    fn gemini_normal_response() {
        let body = r#"{
            "candidates": [{
                "content": {"parts": [{"text": "hello world"}], "role": "model"},
                "finishReason": "STOP"
            }]
        }"#;
        let text = extract_gemini_text(body).unwrap();
        assert_eq!(text, "hello world");
    }

    #[test]
    fn gemini_error_response() {
        let body = r#"{
            "error": {
                "code": 429,
                "message": "You exceeded your current quota",
                "status": "RESOURCE_EXHAUSTED"
            }
        }"#;
        let err = extract_gemini_text(body).unwrap_err();
        assert!(err.to_string().contains("429"));
        assert!(err.to_string().contains("quota"));
    }

    #[test]
    fn gemini_safety_blocked() {
        let body = r#"{
            "candidates": [{
                "finishReason": "SAFETY",
                "content": {"parts": [{"text": ""}], "role": "model"}
            }]
        }"#;
        let err = extract_gemini_text(body).unwrap_err();
        assert!(err.to_string().contains("안전 필터"));
    }

    #[test]
    fn gemini_empty_candidates() {
        let body = r#"{"candidates": []}"#;
        let err = extract_gemini_text(body).unwrap_err();
        assert!(err.to_string().contains("비어있습니다"));
    }

    #[test]
    fn gemini_invalid_json() {
        assert!(extract_gemini_text("not json at all").is_err());
    }

    #[test]
    fn gemini_missing_text_field() {
        let body = r#"{"candidates": [{"content": {"parts": []}}]}"#;
        let err = extract_gemini_text(body).unwrap_err();
        assert!(err.to_string().contains("비어있습니다"));
    }
}
