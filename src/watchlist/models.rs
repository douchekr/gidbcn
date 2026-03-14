use serde::{Deserialize, Serialize};

/// Gemini가 추천한 후보 종목
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: i64,
    pub ticker: String,
    pub name: String,
    pub sector: String,
    pub reason: String,
    pub score: Option<f64>,
    pub verdict: Option<String>,
    pub status: CandidateStatus,
    pub prompt_id: Option<i64>,
    pub created_at: String,
    pub judged_at: Option<String>,
    pub detail_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    Pending,
    Collected,
    Judged,
    Blacklisted,
}

impl CandidateStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Collected => "collected",
            Self::Judged => "judged",
            Self::Blacklisted => "blacklisted",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "collected" => Self::Collected,
            "judged" => Self::Judged,
            "blacklisted" => Self::Blacklisted,
            _ => Self::Pending,
        }
    }
}

/// 블랙리스트 종목
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BlacklistEntry {
    pub id: i64,
    pub ticker: String,
    pub reason: String,
    pub added_at: String,
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

/// Gemini 사냥 결과 (JSON 파싱용)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HuntResult {
    pub ticker: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub sector: String,
    #[serde(default)]
    pub reason: String,
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
