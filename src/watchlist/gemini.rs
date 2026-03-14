use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::db;
use super::models::{HuntResult, JudgeResult, PromptType};
use crate::storage;

/// Gemini API raw 호출
async fn call_gemini(client: &reqwest::Client, prompt: &str) -> Result<String> {
    let (api_key, model) = storage::with_config(|c| {
        (c.watchlist.gemini_api_key.clone(), c.watchlist.gemini_model.clone())
    });

    if api_key.is_empty() {
        bail!("watchlist.gemini_api_key가 설정되지 않았습니다");
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
        .context("Gemini API 요청 실패")?;

    let status = resp.status();
    let text = resp.text().await.context("Gemini 응답 읽기 실패")?;

    if !status.is_success() {
        bail!("Gemini API 오류 ({status}): {text}");
    }

    // 응답에서 텍스트 추출
    let json: Value = serde_json::from_str(&text).context("Gemini 응답 JSON 파싱 실패")?;
    let content = json["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string();

    Ok(content)
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

/// 사냥: 프롬프트로 후보 종목 수집
pub async fn hunt(client: &reqwest::Client) -> Result<Vec<HuntResult>> {
    // 프롬프트 확인
    let hunt_prompt = db::get_prompt(PromptType::Hunt)?
        .context("사냥 프롬프트가 설정되지 않았습니다. /w prompt hunt set 으로 설정하세요")?;

    // 일일 호출 제한 체크
    let max_calls = storage::with_config(|c| c.watchlist.max_gemini_calls_per_day);
    let today_calls = db::gemini_calls_today()?;
    if today_calls >= max_calls {
        bail!("오늘 Gemini 호출 한도 초과 ({today_calls}/{max_calls})");
    }

    let candidate_count = storage::with_config(|c| c.watchlist.candidate_count);

    // 블랙리스트 + 기존 후보 맥락 구성
    let blacklist = db::list_blacklist()?;
    let bl_tickers: Vec<String> = blacklist.iter().map(|b| b.ticker.clone()).collect();

    let full_prompt = format!(
        "{hunt_prompt}\n\n\
         Return exactly {candidate_count} tickers as a JSON array:\n\
         [{{\"ticker\":\"XXX\",\"name\":\"...\",\"sector\":\"...\",\"reason\":\"...\"}}]\n\
         No other text, no markdown.\n\n\
         Exclude these blacklisted tickers: {bl_list}",
        bl_list = if bl_tickers.is_empty() { "none".to_string() } else { bl_tickers.join(", ") },
    );

    // Gemini 호출
    let response = call_gemini(client, &full_prompt).await;
    let model = storage::with_config(|c| c.watchlist.gemini_model.clone());

    // API 사용 로그
    let _ = db::log_api_call("gemini", "hunt", response.is_ok());

    let response_text = response?;

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
            // 파싱 실패해도 이력은 저장
            let _ = db::insert_prompt_history(
                PromptType::Hunt, &full_prompt, &response_text, &model, "", "parse_error",
            );
            bail!("사냥 결과 파싱 실패: {e}");
        }
    }

    // 이력 저장
    let prompt_id = db::insert_prompt_history(
        PromptType::Hunt, &full_prompt, &response_text, &model, &tickers_str, "success",
    )?;

    // 블랙리스트 필터링 + DB 저장
    let mut saved = Vec::new();
    for r in &results {
        let ticker = r.ticker.to_uppercase();
        if db::is_blacklisted(&ticker)? {
            tracing::info!("사냥 결과 블랙리스트 제외: {ticker}");
            continue;
        }
        let _ = db::insert_candidate(&ticker, &r.name, &r.sector, &r.reason, Some(prompt_id));
        saved.push(r.clone());
    }

    tracing::info!("사냥 완료: {}개 후보 저장 ({}개 블랙리스트 제외)",
        saved.len(), results.len() - saved.len());

    Ok(saved)
}

/// 처단: 한투 데이터 기반으로 Gemini에게 평가 요청
pub async fn judge(
    client: &reqwest::Client,
    data_text: &str,
) -> Result<Vec<JudgeResult>> {
    // 프롬프트 확인
    let judge_prompt = db::get_prompt(PromptType::Judge)?
        .context("처단 프롬프트가 설정되지 않았습니다. /w prompt judge set 으로 설정하세요")?;

    // 일일 호출 제한 체크
    let max_calls = storage::with_config(|c| c.watchlist.max_gemini_calls_per_day);
    let today_calls = db::gemini_calls_today()?;
    if today_calls >= max_calls {
        bail!("오늘 Gemini 호출 한도 초과 ({today_calls}/{max_calls})");
    }

    let full_prompt = format!(
        "{judge_prompt}\n\n\
         Here is the real market data for each stock:\n\
         {data_text}\n\n\
         Return a JSON array with your evaluation:\n\
         [{{\"ticker\":\"XXX\",\"score\":85,\"verdict\":\"...\"}}]\n\
         score: 0-100, verdict: brief explanation.\n\
         No other text, no markdown."
    );

    // Gemini 호출
    let response = call_gemini(client, &full_prompt).await;
    let model = storage::with_config(|c| c.watchlist.gemini_model.clone());

    let _ = db::log_api_call("gemini", "judge", response.is_ok());

    let response_text = response?;

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
                PromptType::Judge, &full_prompt, &response_text, &model, "", "parse_error",
            );
            bail!("처단 결과 파싱 실패: {e}");
        }
    }

    let _ = db::insert_prompt_history(
        PromptType::Judge, &full_prompt, &response_text, &model, &tickers_str, "success",
    );

    tracing::info!("처단 완료: {}개 종목 평가", results.len());

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
}
