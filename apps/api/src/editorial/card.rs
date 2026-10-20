//! 一图流日报：把一期日报渲染成一张竖版 PNG，手机上一屏一屏往下看，也方便转发。
//! 先由代码拼 SVG（手工按字宽折行），再用 resvg 栅格化；不调用模型，同一期内容渲染结果确定。
//! 字体用系统里的 Noto Sans CJK（镜像里装 fonts-noto-cjk），本机开发回退到苹方。

use std::sync::OnceLock;

use chrono::{Datelike, NaiveDate};
use resvg::{tiny_skia, usvg};

use super::edition::{EditionItem, grouped};
use crate::error::{AppError, AppResult};

const WIDTH: f32 = 1080.0;
const MARGIN: f32 = 64.0;
const CONTENT: f32 = WIDTH - MARGIN * 2.0;
const NUMBER_COLUMN: f32 = 64.0;
const PADDING: f32 = 28.0;
/// 文字从序号列右侧开始，到卡片右内边距为止。
const TEXT_LEFT: f32 = MARGIN + PADDING + NUMBER_COLUMN - 8.0;
const TEXT_WIDTH: f32 = MARGIN + CONTENT - PADDING - TEXT_LEFT;
const HEADLINE_SIZE: f32 = 38.0;
const HEADLINE_LINE: f32 = 52.0;
const REASON_SIZE: f32 = 28.0;
const REASON_LINE: f32 = 40.0;
const META_SIZE: f32 = 24.0;
const MAX_HEADLINE_LINES: usize = 2;
const MAX_REASON_LINES: usize = 2;
const FONT: &str = "Noto Sans CJK SC, PingFang SC, Hiragino Sans GB, sans-serif";

const INK: &str = "#1d1d1f";
const SECONDARY: &str = "#6e6e73";
const TERTIARY: &str = "#86868b";
const CANVAS: &str = "#f5f5f7";
const SURFACE: &str = "#ffffff";
const HAIRLINE: &str = "#e5e5ea";
const ACCENT: &str = "#2f5fd0";

/// 渲染一期日报。`public_host` 只用于页脚署名。
pub fn render_png(date: NaiveDate, items: &[EditionItem], public_host: &str) -> AppResult<Vec<u8>> {
    let svg = render_svg(date, items, public_host);
    let options = usvg::Options {
        fontdb: fonts().clone(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(&svg, &options)
        .map_err(|error| AppError::Anyhow(anyhow::anyhow!("card svg: {error}")))?;
    let size = tree.size().to_int_size();
    let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height())
        .ok_or_else(|| AppError::Anyhow(anyhow::anyhow!("card pixmap is empty")))?;
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    pixmap
        .encode_png()
        .map_err(|error| AppError::Anyhow(anyhow::anyhow!("card png: {error}")))
}

/// 系统字体只扫描一次：扫描 Noto CJK 的 ttc 要几百毫秒。
fn fonts() -> &'static std::sync::Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<std::sync::Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut database = usvg::fontdb::Database::new();
        database.load_system_fonts();
        std::sync::Arc::new(database)
    })
}

pub fn render_svg(date: NaiveDate, items: &[EditionItem], public_host: &str) -> String {
    let mut body = String::new();
    let mut y = 0.0;

    // 页眉：深色实底，日期和规模。
    let header_height = 236.0;
    let source_count = items
        .iter()
        .flat_map(|item| item.sources.iter())
        .collect::<std::collections::HashSet<_>>()
        .len();
    body.push_str(&format!(
        r##"<rect x="0" y="0" width="{WIDTH}" height="{header_height}" fill="{INK}"/>
<text x="{MARGIN}" y="104" font-size="64" font-weight="700" fill="#ffffff">情报日报</text>
<text x="{MARGIN}" y="164" font-size="32" fill="#d2d2d7">{}</text>
<text x="{MARGIN}" y="208" font-size="26" fill="{TERTIARY}">过去 24 小时 · {} 条要闻 · {source_count} 个信源</text>
"##,
        date_label(date),
        items.len()
    ));
    y += header_height + 24.0;

    let mut number = 0;
    for (label, section_items) in grouped(items) {
        y += 40.0;
        body.push_str(&format!(
            r##"<text x="{MARGIN}" y="{y}" font-size="26" font-weight="700" fill="{ACCENT}" letter-spacing="2">{}</text>
"##,
            escape(label)
        ));
        y += 20.0;
        for item in section_items {
            number += 1;
            let headline = clean(item.headline.as_deref().unwrap_or(&item.title));
            let headline_lines = wrap(&headline, HEADLINE_SIZE, TEXT_WIDTH, MAX_HEADLINE_LINES);
            let reason = if item.why_it_matters.trim().is_empty() {
                item.what_happened.as_str()
            } else {
                item.why_it_matters.as_str()
            };
            let reason_lines = wrap(&clean(reason), REASON_SIZE, TEXT_WIDTH, MAX_REASON_LINES);
            let card_height = 36.0
                + headline_lines.len() as f32 * HEADLINE_LINE
                + 8.0
                + reason_lines.len() as f32 * REASON_LINE
                + 12.0
                + META_SIZE
                + 32.0;
            body.push_str(&format!(
                r##"<rect x="{MARGIN}" y="{y}" width="{CONTENT}" height="{card_height}" rx="10" fill="{SURFACE}" stroke="{HAIRLINE}"/>
"##
            ));
            let number_right = TEXT_LEFT - 14.0;
            let text_left = TEXT_LEFT;
            let mut line_y = y + 36.0 + HEADLINE_SIZE * 0.85;
            body.push_str(&format!(
                r##"<text x="{number_right}" y="{line_y}" font-size="{HEADLINE_SIZE}" font-weight="700" fill="{TERTIARY}" text-anchor="end">{number}</text>
"##
            ));
            for line in &headline_lines {
                body.push_str(&format!(
                    r##"<text x="{text_left}" y="{line_y}" font-size="{HEADLINE_SIZE}" font-weight="700" fill="{INK}">{}</text>
"##,
                    escape(line)
                ));
                line_y += HEADLINE_LINE;
            }
            line_y += 8.0 - HEADLINE_LINE + REASON_LINE;
            for line in &reason_lines {
                body.push_str(&format!(
                    r##"<text x="{text_left}" y="{line_y}" font-size="{REASON_SIZE}" fill="{SECONDARY}">{}</text>
"##,
                    escape(line)
                ));
                line_y += REASON_LINE;
            }
            line_y += 12.0 - REASON_LINE + META_SIZE + 4.0;
            body.push_str(&format!(
                r##"<text x="{text_left}" y="{line_y}" font-size="{META_SIZE}" fill="{TERTIARY}">{}</text>
"##,
                escape(&meta(item))
            ));
            y += card_height + 16.0;
        }
    }

    y += 32.0;
    body.push_str(&format!(
        r##"<text x="{MARGIN}" y="{y}" font-size="24" fill="{TERTIARY}">{} · 由评审分与信源数自动挑选</text>
"##,
        escape(public_host)
    ));
    let height = (y + 56.0).ceil();
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{height}" viewBox="0 0 {WIDTH} {height}" font-family="{FONT}">
<rect width="100%" height="100%" fill="{CANVAS}"/>
{body}</svg>"##
    )
}

fn date_label(date: NaiveDate) -> String {
    const WEEKDAYS: [&str; 7] = ["一", "二", "三", "四", "五", "六", "日"];
    format!(
        "{} 年 {} 月 {} 日 · 星期{}",
        date.year(),
        date.month(),
        date.day(),
        WEEKDAYS[date.weekday().num_days_from_monday() as usize]
    )
}

fn meta(item: &EditionItem) -> String {
    let mut parts = vec![format!("{} 个信源", item.sources.len().max(1))];
    if let Some(score) = item.editorial_score {
        parts.push(format!("评审 {score:.1}"));
    }
    parts.join(" · ")
}

/// 订阅源标题常带 HTML 实体和频道前缀符号（🖼、↩️、【）；图里只留文字。
fn clean(value: &str) -> String {
    let decoded = decode_entities(value);
    decoded
        .trim_start_matches(|character: char| {
            !(character.is_alphanumeric() || "“‘\"'（(《【".contains(character))
        })
        .trim()
        .to_string()
}

fn decode_entities(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(end) = tail[..tail.len().min(12)].find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(character) => {
                out.push(character);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 估算字宽：中日韩和全角字符 1 个字号，其余按 0.56 个字号（拉丁、数字的平均宽度）。
fn char_width(character: char, size: f32) -> f32 {
    let wide = matches!(character as u32,
        0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6);
    // 拉丁字母按 0.62 个字号估（Noto Sans 粗体大写偏宽），宁可早折行也不出界。
    if wide {
        size
    } else if character == ' ' {
        size * 0.3
    } else if character.is_ascii_uppercase() {
        size * 0.7
    } else {
        size * 0.6
    }
}

/// 按宽度折行，最多 `max_lines` 行，超出时末行以省略号结尾。英文单词尽量不拆开。
fn wrap(text: &str, size: f32, width: f32, max_lines: usize) -> Vec<String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut lines = Vec::<String>::new();
    let mut current = String::new();
    let mut current_width = 0.0;
    let mut truncated = false;
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        // 拉丁单词整体放置：先量出整个单词。
        let mut token = character.to_string();
        if character.is_ascii_alphanumeric() {
            while let Some(next) = chars.peek().copied().filter(|next| next.is_ascii_alphanumeric()) {
                token.push(next);
                chars.next();
            }
        }
        let token_width = token.chars().map(|value| char_width(value, size)).sum::<f32>();
        if current_width + token_width > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current).trim_end().to_string());
            current_width = 0.0;
            if lines.len() == max_lines {
                truncated = true;
                break;
            }
            if token == " " {
                continue;
            }
        }
        current.push_str(&token);
        current_width += token_width;
    }
    if !truncated && !current.trim().is_empty() {
        lines.push(current.trim_end().to_string());
    }
    if truncated && let Some(last) = lines.last_mut() {
        let ellipsis = char_width('…', size);
        let mut used = last.chars().map(|value| char_width(value, size)).sum::<f32>();
        while used + ellipsis > width && last.pop().is_some() {
            used = last.chars().map(|value| char_width(value, size)).sum::<f32>();
        }
        last.push('…');
    }
    lines
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_decodes_entities_and_strips_channel_prefixes() {
        assert_eq!(clean("Musk &#8216;Trump Accounts&#8217; &amp; more"), "Musk ‘Trump Accounts’ & more");
        assert_eq!(clean("🖼 Anthropic宣布"), "Anthropic宣布");
        assert_eq!(clean("↩️🖼 🍏 “武汉通”上线"), "“武汉通”上线");
        assert_eq!(clean("【三菱电机】合作"), "【三菱电机】合作");
        assert_eq!(clean("a &bogus b"), "a &bogus b");
    }

    #[test]
    fn wrap_respects_width_and_line_limit() {
        let lines = wrap(&"法国多地骚乱蔓延".repeat(10), 38.0, 380.0, 2);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|line| line.chars().count() <= 10));
        assert!(lines[1].ends_with('…'));
        let english = wrap("Trump suspends some Russia sanctions by executive order", 30.0, 400.0, 3);
        assert!(english.iter().all(|line| !line.starts_with(' ')));
        assert!(english.concat().contains("sanctions"), "words are not split: {english:?}");
        assert_eq!(wrap("短标题", 38.0, 900.0, 2), vec!["短标题"]);
    }

    #[test]
    fn svg_escapes_text_and_is_parseable() {
        let item = EditionItem {
            event_id: uuid::Uuid::new_v4(),
            section: "politics".to_string(),
            title: "A & B <script>".to_string(),
            headline: None,
            what_happened: "发生了什么".to_string(),
            why_it_matters: "影响两国边境居民".to_string(),
            history_note: None,
            history_refs: Vec::new(),
            community_note: None,
            takeaway: None,
            sources: vec!["BBC".to_string(), "AP".to_string()],
            confidence_label: "多源印证".to_string(),
            editorial_score: Some(7.5),
        };
        let date = NaiveDate::from_ymd_opt(2026, 10, 21).unwrap();
        let svg = render_svg(date, std::slice::from_ref(&item), "infofetcher.error.moe");
        assert!(svg.contains("A &amp; B &lt;script&gt;"));
        assert!(svg.contains("星期三"));
        assert!(usvg::Tree::from_str(&svg, &usvg::Options::default()).is_ok());
        let png = render_png(date, &[item], "infofetcher.error.moe").unwrap();
        assert_eq!(&png[1..4], b"PNG");
    }
}

/// 手动预览：`CARD_JSON=<日报 JSON> CARD_OUT=<png> cargo test --bins card::preview -- --ignored`。
#[cfg(test)]
mod preview {
    #[test]
    #[ignore]
    fn render_edition_json() {
        let json = std::fs::read_to_string(std::env::var("CARD_JSON").unwrap()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let items: Vec<super::EditionItem> = serde_json::from_value(value["items"].clone()).unwrap();
        let date = chrono::NaiveDate::parse_from_str(value["edition_date"].as_str().unwrap(), "%Y-%m-%d").unwrap();
        let png = super::render_png(date, &items, "infofetcher.error.moe").unwrap();
        std::fs::write(std::env::var("CARD_OUT").unwrap(), png).unwrap();
    }
}
