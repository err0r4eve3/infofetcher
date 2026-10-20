use serde_json::json;

use crate::{
    config::Config,
    error::{AppError, AppResult},
};

pub struct PushMessage<'a> {
    pub rule_name: &'a str,
    pub matched_keywords: &'a [String],
    pub article_title: &'a str,
    pub feed_title: &'a str,
    pub summary: Option<&'a str>,
    pub article_url: &'a str,
}

pub fn build_telegram_message(message: &PushMessage<'_>) -> String {
    let mut lines = vec![
        format!("关键词命中：{}", message.rule_name),
        format!("命中词：{}", message.matched_keywords.join(", ")),
        format!("文章：{}", message.article_title),
        format!("订阅：{}", message.feed_title),
    ];
    if let Some(summary) = message
        .summary
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        lines.push(format!("摘要：{}", clip_message_part(summary, 700)));
    }
    lines.push(message.article_url.to_string());
    lines.join("\n")
}

pub async fn send_telegram(
    config: &Config,
    http: &reqwest::Client,
    message: &str,
) -> AppResult<()> {
    if config.telegram_bot_token.trim().is_empty() || config.telegram_chat_id.trim().is_empty() {
        return Err(AppError::BadRequest(
            "TELEGRAM_BOT_TOKEN 和 TELEGRAM_CHAT_ID 未配置".to_string(),
        ));
    }
    let endpoint = format!(
        "{}/bot{}/sendMessage",
        config.telegram_api_base.trim_end_matches('/'),
        config.telegram_bot_token
    );
    let response = http
        .post(endpoint)
        .json(&json!({
            "chat_id": config.telegram_chat_id,
            "text": message,
            "disable_web_page_preview": false
        }))
        .timeout(config.telegram_timeout)
        .send()
        .await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::Upstream(format!(
            "Telegram 推送失败: {status} {body}"
        )));
    }
    Ok(())
}

/// 发一张图片，`caption` 不超过 Telegram 的 1024 字符上限。
pub async fn send_telegram_photo(
    config: &Config,
    http: &reqwest::Client,
    png: Vec<u8>,
    caption: &str,
) -> AppResult<()> {
    if config.telegram_bot_token.trim().is_empty() || config.telegram_chat_id.trim().is_empty() {
        return Err(AppError::BadRequest(
            "TELEGRAM_BOT_TOKEN 和 TELEGRAM_CHAT_ID 未配置".to_string(),
        ));
    }
    let endpoint = format!(
        "{}/bot{}/sendPhoto",
        config.telegram_api_base.trim_end_matches('/'),
        config.telegram_bot_token
    );
    let photo = reqwest::multipart::Part::bytes(png)
        .file_name("brief.png")
        .mime_str("image/png")?;
    let form = reqwest::multipart::Form::new()
        .text("chat_id", config.telegram_chat_id.clone())
        .text("caption", caption.chars().take(1_000).collect::<String>())
        .part("photo", photo);
    let response = http
        .post(endpoint)
        .multipart(form)
        .timeout(config.telegram_timeout * 3)
        .send()
        .await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::Upstream(format!(
            "Telegram 发图失败: {status} {body}"
        )));
    }
    Ok(())
}

fn clip_message_part(value: &str, limit: usize) -> String {
    let mut clipped = value.chars().take(limit).collect::<String>();
    if value.chars().count() > limit {
        clipped.push_str("...");
    }
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_keyword_match_for_telegram() {
        let message = build_telegram_message(&PushMessage {
            rule_name: "AI 观察",
            matched_keywords: &["LLM".to_string(), "OpenAI".to_string()],
            article_title: "OpenAI 发布新模型",
            feed_title: "Infofetcher",
            summary: Some("这是摘要"),
            article_url: "https://example.com/article",
        });

        assert!(message.contains("AI 观察"));
        assert!(message.contains("LLM, OpenAI"));
        assert!(message.contains("OpenAI 发布新模型"));
        assert!(message.contains("这是摘要"));
        assert!(message.contains("https://example.com/article"));
    }
}
