//! 日报：每天定时把 24 小时简报定格为一期，由代码渲染成 Markdown 和 HTML，不再调用模型。
//! 提供 Atom 订阅，并可推送到 Webhook（通用 JSON 或飞书）和 Telegram。

use chrono::{DateTime, FixedOffset, NaiveDate, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    config::Config,
    error::{AppError, AppResult},
    push::{send_telegram, send_telegram_photo},
    state::AppState,
    world::{self, EventBriefItem, HistoryRef},
};

pub const WINDOW_HOURS: i64 = 24;
pub const MAX_ITEMS: usize = 20;
pub const FEED_EDITIONS: i64 = 30;
const PUSH_ITEMS: usize = 10;

use crate::world::REPORT_SECTIONS as SECTIONS;

/// 2026-10-20 栏目细分前的旧键，旧日报按它们归到新栏目。
fn current_section(key: &str) -> &str {
    match key {
        "world" => "politics",
        "technology" => "tech",
        "security" => "cyber",
        other => other,
    }
}

/// 一期日报里的一条，独立于简报结构保存，旧日报不受接口字段变化影响。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EditionItem {
    pub event_id: Uuid,
    pub section: String,
    pub title: String,
    /// 评审写的中文短标题；旧日报没有这个字段。
    #[serde(default)]
    pub headline: Option<String>,
    pub what_happened: String,
    pub why_it_matters: String,
    pub history_note: Option<String>,
    pub history_refs: Vec<HistoryRef>,
    pub community_note: Option<String>,
    pub takeaway: Option<String>,
    pub sources: Vec<String>,
    pub confidence_label: String,
    pub editorial_score: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Edition {
    pub edition_date: NaiveDate,
    pub generated_at: DateTime<Utc>,
    pub item_count: i32,
    pub markdown: String,
    pub items: Vec<EditionItem>,
}

impl From<&EventBriefItem> for EditionItem {
    fn from(item: &EventBriefItem) -> Self {
        Self {
            event_id: item.event_id,
            section: item.report_section.clone(),
            title: one_line(&item.title, TITLE_CHARS),
            headline: item.headline.clone(),
            what_happened: one_line(&item.what_happened, SUMMARY_CHARS),
            // 未评审时“为什么重要”是排序模板语，不进日报。
            why_it_matters: if item.editorial_score.is_some() {
                one_line(&item.why_it_matters, SUMMARY_CHARS)
            } else {
                String::new()
            },
            history_note: item.history_note.clone(),
            history_refs: item.history_refs.clone(),
            community_note: item.community_note.clone(),
            takeaway: item.takeaway.clone(),
            sources: item.source_titles.clone(),
            confidence_label: item.confidence_label.clone(),
            editorial_score: item.editorial_score,
        }
    }
}

const TITLE_CHARS: usize = 120;
const SUMMARY_CHARS: usize = 400;

/// 社媒条目的标题可能是多行原文；日报里压成一行并截断。
fn one_line(value: &str, max_chars: usize) -> String {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = collapsed.chars();
    let clipped = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{clipped}…")
    } else {
        clipped
    }
}

pub fn local_offset(config: &Config) -> FixedOffset {
    FixedOffset::east_opt(config.editorial.edition_utc_offset_hours * 3_600)
        .unwrap_or_else(|| FixedOffset::east_opt(0).expect("zero offset"))
}

/// 到点且当天还没有日报时生成并推送。返回是否生成了新一期。
pub async fn maybe_publish(state: &AppState) -> AppResult<bool> {
    let local_now = Utc::now().with_timezone(&local_offset(&state.config));
    if local_now.hour() < state.config.editorial.edition_hour {
        return Ok(false);
    }
    let date = local_now.date_naive();
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM brief_editions WHERE edition_date = $1)",
    )
    .bind(date)
    .fetch_one(&state.pool)
    .await?;
    if exists {
        return Ok(false);
    }
    let edition = publish(state, date).await?;
    deliver(state, &edition).await;
    Ok(true)
}

pub async fn publish(state: &AppState, date: NaiveDate) -> AppResult<Edition> {
    let brief = world::build_brief(state, WINDOW_HOURS, MAX_ITEMS, None).await?;
    let items = brief
        .items
        .iter()
        .map(EditionItem::from)
        .collect::<Vec<_>>();
    let markdown = render_markdown(
        date,
        &items,
        brief.stats.candidate_events,
        &state.config.app_public_url,
    );
    let generated_at = Utc::now();
    sqlx::query(
        "INSERT INTO brief_editions (edition_date, window_hours, item_count, markdown, items, generated_at)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (edition_date) DO UPDATE SET
           window_hours = EXCLUDED.window_hours, item_count = EXCLUDED.item_count,
           markdown = EXCLUDED.markdown, items = EXCLUDED.items,
           generated_at = EXCLUDED.generated_at, delivered_at = NULL, delivery_error = NULL",
    )
    .bind(date)
    .bind(WINDOW_HOURS as i32)
    .bind(items.len() as i32)
    .bind(&markdown)
    .bind(json!(items))
    .bind(generated_at)
    .execute(&state.pool)
    .await?;
    tracing::info!(%date, items = items.len(), "brief edition published");
    Ok(Edition {
        edition_date: date,
        generated_at,
        item_count: items.len() as i32,
        markdown,
        items,
    })
}

pub async fn latest(state: &AppState, limit: i64) -> AppResult<Vec<Edition>> {
    let rows = sqlx::query(
        "SELECT edition_date, generated_at, item_count, markdown, items
         FROM brief_editions ORDER BY edition_date DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows.into_iter().map(edition_from_row).collect())
}

pub async fn by_date(state: &AppState, date: NaiveDate) -> AppResult<Option<Edition>> {
    let row = sqlx::query(
        "SELECT edition_date, generated_at, item_count, markdown, items
         FROM brief_editions WHERE edition_date = $1",
    )
    .bind(date)
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.map(edition_from_row))
}

fn edition_from_row(row: sqlx::postgres::PgRow) -> Edition {
    Edition {
        edition_date: row.get("edition_date"),
        generated_at: row.get("generated_at"),
        item_count: row.get("item_count"),
        markdown: row.get("markdown"),
        items: serde_json::from_value(row.get("items")).unwrap_or_default(),
    }
}

/// 渲染一期的一图流。栅格化是纯 CPU 计算，放到阻塞线程池。
pub async fn card_png(state: &AppState, edition: &Edition) -> AppResult<Vec<u8>> {
    let items = edition.items.clone();
    let date = edition.edition_date;
    let host = public_host(&state.config.app_public_url);
    tokio::task::spawn_blocking(move || super::card::render_png(date, &items, &host))
        .await
        .map_err(|error| AppError::Anyhow(error.into()))?
}

fn public_host(public_url: &str) -> String {
    public_url
        .trim_end_matches('/')
        .split("://")
        .last()
        .unwrap_or(public_url)
        .to_string()
}

pub fn card_feed_url(public_url: &str, date: NaiveDate, token: &str) -> String {
    format!(
        "{}/feeds/brief/{}.png?token={}",
        public_url.trim_end_matches('/'),
        date.format("%Y-%m-%d"),
        url_encode(token)
    )
}

fn url_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

pub fn event_url(public_url: &str, event_id: Uuid) -> String {
    format!("{}/?event={event_id}", public_url.trim_end_matches('/'))
}

fn section_label(key: &str) -> &'static str {
    let key = current_section(key);
    SECTIONS
        .iter()
        .find(|(section, _)| *section == key)
        .map_or("其他", |(_, label)| label)
}

/// 按固定栏目顺序分组的条目。
pub(super) fn grouped(items: &[EditionItem]) -> Vec<(&'static str, Vec<&EditionItem>)> {
    SECTIONS
        .iter()
        .map(|(key, label)| {
            (
                *label,
                items
                    .iter()
                    .filter(|item| {
                        let section = current_section(&item.section);
                        section == *key
                            || (*key == "general" && !SECTIONS.iter().any(|(k, _)| *k == section))
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .filter(|(_, items)| !items.is_empty())
        .collect()
}

fn escape_markdown(value: &str) -> String {
    value.replace('[', "\\[").replace(']', "\\]")
}

pub fn render_markdown(
    date: NaiveDate,
    items: &[EditionItem],
    candidates: usize,
    public_url: &str,
) -> String {
    let mut out = format!(
        "# 信息简报 · {}\n\n从 {candidates} 个事件中选出 {} 条。\n",
        date.format("%Y-%m-%d"),
        items.len()
    );
    for (label, section_items) in grouped(items) {
        out.push_str(&format!("\n## {label}\n"));
        for item in section_items {
            out.push_str(&format!(
                "\n### [{}]({})\n\n{}\n",
                escape_markdown(&item.title),
                event_url(public_url, item.event_id),
                item.what_happened.trim(),
            ));
            if !item.why_it_matters.trim().is_empty() {
                out.push_str(&format!(
                    "\n**为什么重要**：{}\n",
                    item.why_it_matters.trim()
                ));
            }
            if let Some(note) = &item.history_note {
                out.push_str(&format!("\n**此前进展**：{note}\n"));
            }
            if let Some(note) = &item.takeaway {
                out.push_str(&format!("\n**方法与启示**：{note}\n"));
            }
            if let Some(note) = &item.community_note {
                out.push_str(&format!("\n**社区观点**：{note}\n"));
            }
            out.push_str(&format!(
                "\n来源：{} · {}",
                item.sources.join("、"),
                item.confidence_label
            ));
            if let Some(score) = item.editorial_score {
                out.push_str(&format!(" · 评审 {score:.1}"));
            }
            out.push('\n');
        }
    }
    out
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn render_html(items: &[EditionItem], public_url: &str) -> String {
    let mut out = String::new();
    for (label, section_items) in grouped(items) {
        out.push_str(&format!("<h2>{}</h2>", escape_html(label)));
        for item in section_items {
            out.push_str(&format!(
                "<h3><a href=\"{}\">{}</a></h3><p>{}</p>",
                escape_html(&event_url(public_url, item.event_id)),
                escape_html(&item.title),
                escape_html(item.what_happened.trim()),
            ));
            if !item.why_it_matters.trim().is_empty() {
                out.push_str(&format!(
                    "<p><strong>为什么重要</strong>：{}</p>",
                    escape_html(item.why_it_matters.trim())
                ));
            }
            for (heading, note) in [
                ("此前进展", &item.history_note),
                ("方法与启示", &item.takeaway),
                ("社区观点", &item.community_note),
            ] {
                if let Some(note) = note {
                    out.push_str(&format!(
                        "<p><strong>{heading}</strong>：{}</p>",
                        escape_html(note)
                    ));
                }
            }
            out.push_str(&format!(
                "<p><small>来源：{} · {}</small></p>",
                escape_html(&item.sources.join("、")),
                escape_html(&item.confidence_label)
            ));
        }
    }
    out
}

/// `image_token` 为订阅 token：有它时每期正文顶部嵌入一图流（阅读器取图片也带不了会话 Cookie）。
pub fn render_atom(
    editions: &[Edition],
    public_url: &str,
    feed_url: &str,
    image_token: Option<&str>,
) -> String {
    let updated = editions
        .first()
        .map_or_else(Utc::now, |edition| edition.generated_at)
        .to_rfc3339();
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<feed xmlns=\"http://www.w3.org/2005/Atom\">\n<title>信息简报</title>\n<id>{}</id>\n<link rel=\"self\" href=\"{}\"/>\n<link href=\"{}\"/>\n<updated>{updated}</updated>\n",
        escape_html(&format!("{}/feeds/brief", public_url.trim_end_matches('/'))),
        escape_html(feed_url),
        escape_html(public_url)
    );
    for edition in editions {
        let date = edition.edition_date.format("%Y-%m-%d");
        out.push_str(&format!(
            "<entry>\n<title>信息简报 · {date}</title>\n<id>{}</id>\n<updated>{}</updated>\n<link href=\"{}\"/>\n<content type=\"html\">{}</content>\n</entry>\n",
            escape_html(&format!("{}/feeds/brief/{date}", public_url.trim_end_matches('/'))),
            edition.generated_at.to_rfc3339(),
            escape_html(public_url),
            escape_html(&format!(
                "{}{}",
                image_token.map_or_else(String::new, |token| format!(
                    "<p><img src=\"{}\" alt=\"一图流日报 {date}\"/></p>",
                    escape_html(&card_feed_url(public_url, edition.edition_date, token))
                )),
                render_html(&edition.items, public_url)
            ))
        ));
    }
    out.push_str("</feed>\n");
    out
}

/// 推送用的短文本：前 10 条标题加链接。
pub fn push_text(edition: &Edition, public_url: &str) -> String {
    let mut lines = vec![format!(
        "信息简报 · {}（{} 条）",
        edition.edition_date.format("%Y-%m-%d"),
        edition.item_count
    )];
    for (index, item) in edition.items.iter().take(PUSH_ITEMS).enumerate() {
        lines.push(format!(
            "{}. [{}] {}\n{}",
            index + 1,
            section_label(&item.section),
            item.title,
            event_url(public_url, item.event_id)
        ));
    }
    if edition.items.len() > PUSH_ITEMS {
        lines.push(format!(
            "另有 {} 条见完整简报。",
            edition.items.len() - PUSH_ITEMS
        ));
    }
    lines.join("\n")
}

async fn deliver(state: &AppState, edition: &Edition) {
    let editorial = &state.config.editorial;
    let text = push_text(edition, &state.config.app_public_url);
    let mut errors = Vec::new();
    let mut attempted = false;
    if let Some(url) = &editorial.brief_webhook_url {
        attempted = true;
        let body = if editorial.brief_webhook_format == "feishu" {
            json!({ "msg_type": "text", "content": { "text": text } })
        } else {
            json!({
                "date": edition.edition_date,
                "title": format!("信息简报 · {}", edition.edition_date.format("%Y-%m-%d")),
                "markdown": edition.markdown,
                "items": edition.items,
            })
        };
        if let Err(error) = post_webhook(&state.http, url, &body).await {
            errors.push(format!("webhook: {error}"));
        }
    }
    if editorial.brief_telegram {
        attempted = true;
        // 先发一图流；渲染或发图失败时退回纯文字，保证当天能收到。
        let photo = match card_png(state, edition).await {
            Ok(png) => {
                let caption = format!(
                    "情报日报 · {}\n{}",
                    edition.edition_date.format("%Y-%m-%d"),
                    state.config.app_public_url.trim_end_matches('/')
                );
                send_telegram_photo(&state.config, &state.http, png, &caption).await
            }
            Err(error) => Err(error),
        };
        if let Err(photo_error) = photo {
            tracing::warn!(%photo_error, "brief card delivery failed, sending text");
            if let Err(error) = send_telegram(&state.config, &state.http, &text).await {
                errors.push(format!("telegram: {error}"));
            }
        }
    }
    if !attempted {
        return;
    }
    let error = (!errors.is_empty()).then(|| errors.join("; "));
    if let Some(error) = &error {
        tracing::warn!(%error, "brief edition delivery failed");
    }
    let _ = sqlx::query(
        "UPDATE brief_editions SET delivered_at = CASE WHEN $2::text IS NULL THEN now() END,
                delivery_error = $2
         WHERE edition_date = $1",
    )
    .bind(edition.edition_date)
    .bind(error)
    .execute(&state.pool)
    .await;
}

async fn post_webhook(
    http: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
) -> AppResult<()> {
    let response = http
        .post(url)
        .json(body)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(AppError::Upstream(format!(
            "Webhook 返回 {}",
            response.status()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(section: &str, title: &str) -> EditionItem {
        EditionItem {
            event_id: Uuid::nil(),
            section: section.to_string(),
            title: title.to_string(),
            headline: None,
            what_happened: "发生了什么".to_string(),
            why_it_matters: "影响谁".to_string(),
            history_note: Some("本站 10 月 1 日记录过上一阶段".to_string()),
            history_refs: Vec::new(),
            community_note: None,
            takeaway: None,
            sources: vec!["路透".to_string(), "BBC".to_string()],
            confidence_label: "较高 · 2 个订阅源".to_string(),
            editorial_score: Some(7.5),
        }
    }

    #[test]
    fn markdown_groups_by_section_in_fixed_order() {
        let items = vec![
            item("tech", "芯片 [新规]"),
            item("conflict", "停火协议"),
            item("unknown", "其他"),
            // 旧日报里的旧键归到新栏目。
            item("world", "议会投票"),
        ];
        let markdown = render_markdown(
            NaiveDate::from_ymd_opt(2026, 10, 16).unwrap(),
            &items,
            120,
            "https://x.test/",
        );
        let conflict = markdown.find("## 战争与冲突").unwrap();
        let politics = markdown.find("## 政治与外交").unwrap();
        let tech = markdown.find("## 科技与产品").unwrap();
        let other = markdown.find("## 其他").unwrap();
        assert!(conflict < politics && politics < tech && tech < other);
        assert!(markdown.starts_with("# 信息简报 · 2026-10-16\n\n从 120 个事件中选出 4 条。"));
        assert!(markdown.contains(
            "### [芯片 \\[新规\\]](https://x.test/?event=00000000-0000-0000-0000-000000000000)"
        ));
        assert!(markdown.contains("**此前进展**：本站 10 月 1 日记录过上一阶段"));
        assert!(markdown.contains("来源：路透、BBC · 较高 · 2 个订阅源 · 评审 7.5"));
        assert!(!markdown.contains("**社区观点**"));
    }

    #[test]
    fn atom_escapes_html_content() {
        let edition = Edition {
            edition_date: NaiveDate::from_ymd_opt(2026, 10, 16).unwrap(),
            generated_at: Utc::now(),
            item_count: 1,
            markdown: String::new(),
            items: vec![item("world", "A <b>&</b> B")],
        };
        let atom = render_atom(
            &[edition],
            "https://x.test",
            "https://x.test/feeds/brief.xml?token=t",
            Some("t&u"),
        );
        assert!(atom.contains("https://x.test/feeds/brief/2026-10-16.png?token=t%26u"));
        assert!(atom.contains("<title>信息简报 · 2026-10-16</title>"));
        assert!(atom.contains("A &amp;lt;b&amp;gt;&amp;amp;&amp;lt;/b&amp;gt; B"));
        assert!(!atom.contains("<b>"));
    }

    #[test]
    fn edition_items_flatten_titles_and_drop_template_reasons() {
        assert_eq!(one_line("第一行\n  第二行 ", 120), "第一行 第二行");
        assert_eq!(one_line("abcdef", 3), "abc…");
        let items = vec![EditionItem {
            why_it_matters: String::new(),
            editorial_score: None,
            ..item("world", "无评审")
        }];
        let markdown = render_markdown(
            NaiveDate::from_ymd_opt(2026, 10, 16).unwrap(),
            &items,
            1,
            "https://x.test",
        );
        assert!(!markdown.contains("**为什么重要**"));
        assert!(!render_html(&items, "https://x.test").contains("为什么重要"));
    }

    #[test]
    fn push_text_is_capped() {
        let items = (0..12)
            .map(|index| item("world", &format!("事件 {index}")))
            .collect::<Vec<_>>();
        let edition = Edition {
            edition_date: NaiveDate::from_ymd_opt(2026, 10, 16).unwrap(),
            generated_at: Utc::now(),
            item_count: 12,
            markdown: String::new(),
            items,
        };
        let text = push_text(&edition, "https://x.test");
        assert!(text.starts_with("信息简报 · 2026-10-16（12 条）"));
        assert!(text.contains("10. [政治与外交] 事件 9"), "legacy world key maps to politics");
        assert!(!text.contains("事件 10\n"));
        assert!(text.ends_with("另有 2 条见完整简报。"));
    }
}
