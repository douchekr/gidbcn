use serde::{Deserialize, Serialize};

/// 포획 대기 종목 (pending 테이블)
#[derive(Debug, Clone)]
pub struct PendingEntry {
    pub id: i64,
    pub ticker: String,
    pub market: String,
    pub name: String,
    pub sector: String,
    pub reason: String,
    pub hunt_score: Option<f64>,
    pub hunt_count: i64,
    pub created_at: String,
}

/// 감정 완료 종목 (candidates 테이블, judged/blacklisted)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: i64,
    pub ticker: String,
    pub market: String,
    pub name: String,
    pub sector: String,
    pub reason: String,
    pub hunt_score: Option<f64>,
    pub hunt_count: i64,
    pub score: Option<f64>,
    pub verdict: Option<String>,
    pub detail_text: String,
    pub status: CandidateStatus,
    pub strike_count: i64,
    pub judged_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    Judged,
    Blacklisted,
}

impl CandidateStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Judged => "judged",
            Self::Blacklisted => "blacklisted",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "blacklisted" => Self::Blacklisted,
            _ => Self::Judged,
        }
    }
}

/// Gemini 호출 이력
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PromptRecord {
    pub id: i64,
    pub prompt_type: String,
    pub prompt_text: String,
    pub response_text: String,
    pub model: String,
    pub tickers_extracted: String,
    pub created_at: String,
    pub status: String,
}

/// 사냥 결과 (JSON 파싱용)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HuntResult {
    pub ticker: String,
    #[serde(default)]
    pub market: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub sector: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub score: f64,
}

/// Gemini 평가 결과 (JSON 파싱용)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeResult {
    pub ticker: String,
    pub score: f64,
    #[serde(default)]
    pub verdict: String,
}

/// 프롬프트 타입
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PromptType {
    Hunt,
    Judge,
}

impl PromptType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hunt => "hunt",
            Self::Judge => "judge",
        }
    }

    #[allow(dead_code)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "hunt" => Some(Self::Hunt),
            "judge" => Some(Self::Judge),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_status_roundtrip() {
        for status in &[CandidateStatus::Judged, CandidateStatus::Blacklisted] {
            let s = status.as_str();
            let restored = CandidateStatus::from_str(s);
            assert_eq!(*status, restored);
        }
    }

    #[test]
    fn candidate_status_unknown_defaults_judged() {
        assert_eq!(CandidateStatus::from_str("garbage"), CandidateStatus::Judged);
        assert_eq!(CandidateStatus::from_str(""), CandidateStatus::Judged);
    }

    #[test]
    fn prompt_type_as_str() {
        assert_eq!(PromptType::Hunt.as_str(), "hunt");
        assert_eq!(PromptType::Judge.as_str(), "judge");
    }

    #[test]
    fn prompt_type_from_str() {
        assert_eq!(PromptType::from_str("hunt"), Some(PromptType::Hunt));
        assert_eq!(PromptType::from_str("judge"), Some(PromptType::Judge));
        assert_eq!(PromptType::from_str("unknown"), None);
    }

    #[test]
    fn hunt_result_with_market() {
        let json = r#"{"ticker":"SOUN","market":"NAS","name":"SoundHound","sector":"AI","reason":"voice"}"#;
        let r: HuntResult = serde_json::from_str(json).unwrap();
        assert_eq!(r.ticker, "SOUN");
        assert_eq!(r.market, "NAS");
    }

    #[test]
    fn hunt_result_without_market() {
        let json = r#"{"ticker":"GEVO"}"#;
        let r: HuntResult = serde_json::from_str(json).unwrap();
        assert_eq!(r.ticker, "GEVO");
        assert_eq!(r.market, "");
    }

    #[test]
    fn judge_result_parse() {
        let json = r#"{"ticker":"SOUN","score":85.5,"verdict":"strong buy"}"#;
        let r: JudgeResult = serde_json::from_str(json).unwrap();
        assert_eq!(r.score, 85.5);
    }

    #[test]
    fn candidate_status_serde() {
        let json = serde_json::to_string(&CandidateStatus::Judged).unwrap();
        assert_eq!(json, "\"judged\"");
        let restored: CandidateStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, CandidateStatus::Judged);
    }
}
