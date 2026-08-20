use quick_xml::{Reader, events::Event};
use std::collections::BTreeMap;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpmlFeed {
    pub title: Option<String>,
    pub xml_url: String,
    pub folder_title: Option<String>,
}

pub fn parse_opml(input: &str) -> AppResult<Vec<OpmlFeed>> {
    let mut reader = Reader::from_str(input);
    reader.config_mut().trim_text(true);
    let mut feeds = Vec::new();
    let mut outline_stack: Vec<OutlineContext> = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Empty(event)) if event.name().as_ref() == b"outline" => {
                let (title, xml_url, category) = outline_attrs(&event);
                if let Some(xml_url) = xml_url.filter(|value| !value.trim().is_empty()) {
                    feeds.push(OpmlFeed {
                        title: clean_optional(title),
                        xml_url,
                        folder_title: category.or_else(|| current_folder_title(&outline_stack)),
                    });
                }
            }
            Ok(Event::Start(event)) if event.name().as_ref() == b"outline" => {
                let (title, xml_url, category) = outline_attrs(&event);
                if let Some(xml_url) = xml_url.filter(|value| !value.trim().is_empty()) {
                    feeds.push(OpmlFeed {
                        title: clean_optional(title.clone()),
                        xml_url,
                        folder_title: category.or_else(|| current_folder_title(&outline_stack)),
                    });
                    outline_stack.push(OutlineContext {
                        title: clean_optional(title),
                        is_feed: true,
                    });
                } else {
                    outline_stack.push(OutlineContext {
                        title: clean_optional(title),
                        is_feed: false,
                    });
                }
            }
            Ok(Event::End(event)) if event.name().as_ref() == b"outline" => {
                outline_stack.pop();
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(err) => return Err(AppError::BadRequest(format!("OPML 解析失败: {err}"))),
        }
    }
    Ok(feeds)
}

pub fn render_opml(feeds: &[(String, String, Option<String>)]) -> String {
    let mut uncategorized = Vec::new();
    let mut grouped = BTreeMap::<String, Vec<(String, String)>>::new();

    for (title, url, folder_title) in feeds {
        if let Some(folder_title) = folder_title
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            grouped
                .entry(folder_title.trim().to_string())
                .or_default()
                .push((title.clone(), url.clone()));
        } else {
            uncategorized.push((title.clone(), url.clone()));
        }
    }

    let mut outline_blocks = Vec::new();
    outline_blocks.extend(
        uncategorized
            .iter()
            .map(|(title, url)| render_feed_outline(title, url, 4)),
    );
    outline_blocks.extend(grouped.iter().map(|(folder_title, feeds)| {
        let child_outlines = feeds
            .iter()
            .map(|(title, url)| render_feed_outline(title, url, 6))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            r#"    <outline text="{title}" title="{title}">
{child_outlines}
    </outline>"#,
            title = escape_xml(folder_title)
        )
    }));

    let outlines = outline_blocks.join("\n");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
  <head><title>RSS Hub Reader Export</title></head>
  <body>
{outlines}
  </body>
</opml>
"#
    )
}

fn render_feed_outline(title: &str, url: &str, indent: usize) -> String {
    format!(
        r#"{space}<outline text="{title}" title="{title}" type="rss" xmlUrl="{url}" />"#,
        space = " ".repeat(indent),
        title = escape_xml(title),
        url = escape_xml(url)
    )
}

#[derive(Debug, Clone)]
struct OutlineContext {
    title: Option<String>,
    is_feed: bool,
}

fn outline_attrs(
    event: &quick_xml::events::BytesStart<'_>,
) -> (Option<String>, Option<String>, Option<String>) {
    let mut title = None;
    let mut xml_url = None;
    let mut category = None;
    for attr in event.attributes().flatten() {
        match attr.key.as_ref() {
            b"title" | b"text" if title.is_none() => {
                title = Some(String::from_utf8_lossy(attr.value.as_ref()).to_string());
            }
            b"xmlUrl" | b"xmlurl" => {
                xml_url = Some(String::from_utf8_lossy(attr.value.as_ref()).to_string());
            }
            b"category" if category.is_none() => {
                category = String::from_utf8_lossy(attr.value.as_ref())
                    .split(',')
                    .map(str::trim)
                    .find(|value| !value.is_empty())
                    .map(ToString::to_string);
            }
            _ => {}
        }
    }
    (title, xml_url, category)
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    })
}

fn current_folder_title(stack: &[OutlineContext]) -> Option<String> {
    stack
        .iter()
        .rev()
        .find(|item| !item.is_feed && item.title.is_some())
        .and_then(|item| item.title.clone())
}

fn escape_xml(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_outline_xml_urls() {
        let feeds = parse_opml(
            r#"<opml><body><outline text="A" xmlUrl="https://a.test/feed.xml" /></body></opml>"#,
        )
        .unwrap();
        assert_eq!(
            feeds,
            vec![OpmlFeed {
                title: Some("A".to_string()),
                xml_url: "https://a.test/feed.xml".to_string(),
                folder_title: None
            }]
        );
    }

    #[test]
    fn renders_escaped_opml() {
        let opml = render_opml(&[(
            "A&B".to_string(),
            "https://x.test?a=1&b=2".to_string(),
            None,
        )]);
        assert!(opml.contains("A&amp;B"));
        assert!(opml.contains("a=1&amp;b=2"));
    }

    #[test]
    fn parses_nested_folder_outlines() {
        let feeds = parse_opml(
            r#"<opml><body><outline text="Media"><outline text="BBC" xmlUrl="https://feeds.bbci.co.uk/news/rss.xml" /></outline></body></opml>"#,
        )
        .unwrap();
        assert_eq!(
            feeds,
            vec![OpmlFeed {
                title: Some("BBC".to_string()),
                xml_url: "https://feeds.bbci.co.uk/news/rss.xml".to_string(),
                folder_title: Some("Media".to_string())
            }]
        );
    }

    #[test]
    fn renders_nested_folder_outlines() {
        let opml = render_opml(&[(
            "BBC".to_string(),
            "https://feeds.bbci.co.uk/news/rss.xml".to_string(),
            Some("Media".to_string()),
        )]);
        assert!(opml.contains(r#"<outline text="Media" title="Media">"#));
        assert!(opml.contains(r#"      <outline text="BBC" title="BBC" type="rss" xmlUrl="https://feeds.bbci.co.uk/news/rss.xml" />"#));
    }
}
