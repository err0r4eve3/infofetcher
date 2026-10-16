use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    pub id: Uuid,
    pub title: String,
    pub sort_order: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feed {
    pub id: Uuid,
    pub folder_id: Option<Uuid>,
    pub title: String,
    pub site_url: Option<String>,
    pub feed_url: String,
    pub hub_adapter: Option<String>,
    pub hub_path: Option<String>,
    pub source_type: String,
    pub language: Option<String>,
    pub region: Option<String>,
    pub quality_score: i32,
    pub feedback_weight: i32,
    pub muted: bool,
    pub last_fetched_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub next_fetch_at: Option<DateTime<Utc>>,
    pub consecutive_failures: i32,
    pub last_error: Option<String>,
    pub article_count: i64,
    pub unread_count: i64,
    /// 固定的编辑 Profile；为空时由评审自动判断。
    pub editorial_profile: Option<String>,
    /// 近 30 天的信源画像；尚未计算时为空。
    pub quality_stats: Option<FeedQualityStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedQualityStats {
    /// 评审达标的事件数 / 文章数；文章不足 20 篇时为空。
    pub snr: Option<f32>,
    pub average_score: Option<f32>,
    pub passed_events: i32,
    pub assessed_events: i32,
    pub auto_adjust: i32,
    pub declining: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleListItem {
    pub id: Uuid,
    pub feed_id: Uuid,
    pub feed_title: String,
    pub title: String,
    pub url: String,
    pub author: Option<String>,
    pub excerpt: String,
    pub language: Option<String>,
    pub quality_score: i32,
    pub priority_score: i32,
    pub priority_reasons: Vec<String>,
    pub story_id: Option<Uuid>,
    pub story_source_count: i32,
    pub published_at: Option<DateTime<Utc>>,
    pub fetched_at: DateTime<Utc>,
    pub is_read: bool,
    pub is_favorite: bool,
    pub has_summary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleDetail {
    pub id: Uuid,
    pub feed_id: Uuid,
    pub feed_title: String,
    pub title: String,
    pub url: String,
    pub author: Option<String>,
    pub content: String,
    pub extracted_content: String,
    pub extraction_error: Option<String>,
    pub language: Option<String>,
    pub quality_score: i32,
    pub story_id: Option<Uuid>,
    pub story_source_count: i32,
    pub published_at: Option<DateTime<Utc>>,
    pub fetched_at: DateTime<Utc>,
    pub is_read: bool,
    pub is_favorite: bool,
    pub summary: Option<Summary>,
    pub translation: Option<ArticleTranslation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleTranslation {
    pub article_id: Uuid,
    pub target_language: String,
    pub status: String,
    pub model: String,
    pub source_hash: String,
    pub prompt_version: String,
    pub translated_title: Option<String>,
    pub translated_content: Option<String>,
    pub error: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslationRequest {
    pub article_id: Uuid,
    #[serde(default = "default_translation_target")]
    pub target_language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionTranslationRequest {
    pub text: String,
    pub source_language: Option<String>,
    #[serde(default = "default_translation_target")]
    pub target_language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionTranslationResponse {
    pub translation: String,
    pub target_language: String,
    pub model: String,
}

fn default_translation_target() -> String {
    "zh-Hans".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub article_id: Uuid,
    pub status: String,
    pub model: Option<String>,
    pub schema_version: i32,
    pub prompt_version: String,
    pub method: String,
    pub scope: String,
    pub source_count: i32,
    pub cited_source_count: i32,
    pub source_article_ids: Vec<Uuid>,
    pub input_hash: Option<String>,
    pub summary: Option<String>,
    pub bullets: Vec<String>,
    pub keywords: Vec<String>,
    pub confidence: Option<String>,
    pub confidence_reason: Option<String>,
    pub grounding_score: Option<i32>,
    pub corroboration: Option<String>,
    pub disagreements: Vec<String>,
    pub citations: Vec<SummaryCitation>,
    pub error: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryCitation {
    pub excerpt: String,
    pub claim_id: Option<String>,
    pub evidence_id: Option<String>,
    pub article_id: Option<Uuid>,
    pub source_title: Option<String>,
    pub article_title: Option<String>,
    pub url: Option<String>,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoryCluster {
    pub id: Uuid,
    pub representative_title: String,
    pub source_count: i32,
    pub quality_score: i32,
    pub first_published_at: Option<DateTime<Utc>>,
    pub last_published_at: Option<DateTime<Utc>>,
    pub articles: Vec<ArticleListItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Monitor {
    pub id: Uuid,
    pub name: String,
    pub query: String,
    pub min_quality: i32,
    pub unread_only: bool,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorRequest {
    pub name: String,
    pub query: String,
    #[serde(default)]
    pub min_quality: i32,
    #[serde(default)]
    pub unread_only: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordRule {
    pub id: Uuid,
    pub name: String,
    pub keywords: Vec<String>,
    pub match_title: bool,
    pub match_content: bool,
    pub case_sensitive: bool,
    pub auto_summary: bool,
    pub telegram_enabled: bool,
    pub enabled: bool,
    pub feed_ids: Vec<Uuid>,
    pub folder_ids: Vec<Uuid>,
    pub match_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordMatch {
    pub id: Uuid,
    pub rule_id: Uuid,
    pub rule_name: String,
    pub article_id: Uuid,
    pub feed_id: Uuid,
    pub feed_title: String,
    pub article_title: String,
    pub article_url: String,
    pub excerpt: String,
    pub published_at: Option<DateTime<Utc>>,
    pub fetched_at: DateTime<Utc>,
    pub matched_keywords: Vec<String>,
    pub summary_status: Option<String>,
    pub telegram_status: String,
    pub telegram_error: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchDocument {
    pub id: Uuid,
    pub feed_title: String,
    pub title: String,
    pub content: String,
    pub url: String,
    pub published_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFeedRequest {
    pub feed_url: String,
    pub title: Option<String>,
    pub folder_id: Option<Uuid>,
    pub source_type: Option<String>,
    pub language: Option<String>,
    pub region: Option<String>,
    pub quality_score: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateFeedRequest {
    pub folder_id: Option<Uuid>,
    pub source_type: Option<String>,
    pub language: Option<String>,
    pub region: Option<String>,
    pub quality_score: Option<i32>,
    pub feedback_weight: Option<i32>,
    pub muted: Option<bool>,
    /// `auto` 表示交给评审自动判断。
    pub editorial_profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFolderRequest {
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateArticleRequest {
    pub is_read: Option<bool>,
    pub is_favorite: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryRequest {
    pub article_id: Uuid,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordRuleRequest {
    pub name: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default = "default_true")]
    pub match_title: bool,
    #[serde(default = "default_true")]
    pub match_content: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default = "default_true")]
    pub auto_summary: bool,
    #[serde(default)]
    pub telegram_enabled: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub feed_ids: Vec<Uuid>,
    #[serde(default)]
    pub folder_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordScanRequest {
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordScanResponse {
    pub scanned: i64,
    pub matched: i64,
    pub new_matches: i64,
    pub queued_summaries: i64,
    pub telegram_sent: i64,
    pub telegram_failed: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordMatchActionRequest {
    pub match_ids: Vec<Uuid>,
    pub summarize: bool,
    pub telegram: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordActionResponse {
    pub queued_summaries: i64,
    pub telegram_sent: i64,
    pub telegram_failed: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchSummaryRequest {
    pub folder_id: Option<Uuid>,
    pub feed_id: Option<Uuid>,
    pub unread_only: bool,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobResponse {
    pub queued: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub ok: bool,
    pub service: &'static str,
}
