//! 社区讨论材料：目前只取 Hacker News 前几条顶层评论。评论只进入评审的 `comments`，
//! 不进入摘要证据。

use serde::{Deserialize, Serialize};

const HN_ITEM_URL: &str = "https://hacker-news.firebaseio.com/v0/item";
const TOP_COMMENTS: usize = 5;
const COMMENT_CHARS: usize = 400;

#[derive(Debug, Clone, Serialize)]
pub struct Comment {
    pub author: String,
    pub text: String,
}

#[derive(Debug, Deserialize)]
struct HnItem {
    by: Option<String>,
    text: Option<String>,
    kids: Option<Vec<i64>>,
    deleted: Option<bool>,
    dead: Option<bool>,
}

/// `hn-top` Hub 生成的 guid 形如 `hn-12345`。
pub fn hn_item_id(guid: &str) -> Option<i64> {
    guid.strip_prefix("hn-")?.parse().ok()
}

/// 读取失败不影响评审，返回已取到的评论。
pub async fn hn_top_comments(http: &reqwest::Client, item_id: i64) -> Vec<Comment> {
    let Some(story) = fetch_item(http, item_id).await else {
        return Vec::new();
    };
    let mut comments = Vec::new();
    for kid in story
        .kids
        .unwrap_or_default()
        .into_iter()
        .take(TOP_COMMENTS * 2)
    {
        if comments.len() >= TOP_COMMENTS {
            break;
        }
        let Some(item) = fetch_item(http, kid).await else {
            continue;
        };
        if item.deleted.unwrap_or(false) || item.dead.unwrap_or(false) {
            continue;
        }
        let text = clip(
            &html_to_text(item.text.as_deref().unwrap_or_default()),
            COMMENT_CHARS,
        );
        if text.chars().count() < 20 {
            continue;
        }
        comments.push(Comment {
            author: item.by.unwrap_or_else(|| "匿名".to_string()),
            text,
        });
    }
    comments
}

async fn fetch_item(http: &reqwest::Client, id: i64) -> Option<HnItem> {
    let response = http
        .get(format!("{HN_ITEM_URL}/{id}.json"))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    response.json::<Option<HnItem>>().await.ok().flatten()
}

/// HN 评论是少量内联 HTML：段落、链接、斜体和实体。
fn html_to_text(html: &str) -> String {
    let with_breaks = html.replace("<p>", "\n");
    let mut text = String::with_capacity(with_breaks.len());
    let mut in_tag = false;
    for character in with_breaks.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(character),
            _ => {}
        }
    }
    let decoded = text
        .replace("&#x27;", "'")
        .replace("&#x2F;", "/")
        .replace("&quot;", "\"")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let clipped = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{clipped}…")
    } else {
        clipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hub_guid() {
        assert_eq!(hn_item_id("hn-4242"), Some(4242));
        assert_eq!(hn_item_id("4242"), None);
        assert_eq!(hn_item_id("hn-x"), None);
    }

    #[test]
    fn strips_comment_html() {
        assert_eq!(
            html_to_text("It&#x27;s fast.<p>See <a href=\"x\">docs</a> &amp; <i>notes</i>"),
            "It's fast. See docs & notes"
        );
    }
}
