use scraper::{ElementRef, Html, Selector};
use thiserror::Error;
use tokio::{process::Command, time};

use crate::{
    config::Config,
    error::AppError,
    net::{get_with_safe_redirects, read_response_limited, validate_http_url},
};

#[derive(Debug, Clone)]
pub struct ExtractedContent {
    pub text: String,
    pub error: Option<String>,
}

pub fn content_is_sufficient(text: &str) -> bool {
    normalize_whitespace(text).chars().count() >= 600
}

pub async fn extract_article_content(
    config: &Config,
    http: &reqwest::Client,
    url: &str,
    fallback: &str,
    cookie_header: Option<&str>,
) -> ExtractedContent {
    let normalized_fallback = clean_extracted_text(fallback);
    if feed_content_is_preferred(url) {
        return ExtractedContent {
            text: normalized_fallback.clone(),
            error: if normalized_fallback.is_empty() {
                Some("V2EX Feed 未提供正文，已跳过原文抽取".to_string())
            } else {
                None
            },
        };
    }

    if content_is_sufficient(fallback) {
        return ExtractedContent {
            text: normalized_fallback,
            error: None,
        };
    }

    let direct = fetch_and_extract(config, http, url, cookie_header).await;
    if let Ok(text) = &direct
        && (content_is_sufficient(text) || !config.browser_fetch_enabled)
    {
        return extracted_or_fallback(
            text.clone(),
            normalized_fallback,
            "原文正文为空，已回退到 Feed 内容",
        );
    }

    if config.browser_fetch_enabled {
        match browser_fetch_and_extract(config, url).await {
            Ok(browser_text) if !browser_text.is_empty() => {
                return match &direct {
                    Ok(text) if !browser_text_is_better(text.as_str(), &browser_text) => {
                        ExtractedContent {
                            text: clean_extracted_text(text),
                            error: None,
                        }
                    }
                    _ => ExtractedContent {
                        text: clean_extracted_text(&browser_text),
                        error: None,
                    },
                };
            }
            Ok(_) => {
                if let Ok(text) = &direct {
                    return extracted_or_fallback(
                        text.clone(),
                        normalized_fallback,
                        "浏览器正文为空，已回退到 Feed 内容",
                    );
                }
            }
            Err(browser_err) => {
                if let Ok(text) = &direct
                    && !text.is_empty()
                {
                    return ExtractedContent {
                        text: clean_extracted_text(text),
                        error: Some(format!("{browser_err}，已显示普通抓取内容")),
                    };
                }
            }
        }
    }

    match direct {
        Ok(text) => extracted_or_fallback(
            text,
            normalized_fallback,
            "原文正文为空，已回退到 Feed 内容",
        ),
        Err(ArticleExtractError::Blocked(status)) if !normalized_fallback.is_empty() => {
            ExtractedContent {
                text: normalized_fallback,
                error: Some(format!(
                    "原文站点拒绝自动抓取（{status}），已显示 Feed 内容"
                )),
            }
        }
        Err(err) => ExtractedContent {
            text: normalized_fallback,
            error: Some(err.to_string()),
        },
    }
}

pub fn feed_content_is_preferred(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    matches!(parsed.host_str(), Some("www.v2ex.com" | "v2ex.com"))
}

#[derive(Debug, Error)]
enum ArticleExtractError {
    #[error("正文抓取失败: {0}")]
    Blocked(reqwest::StatusCode),
    #[error(transparent)]
    App(#[from] AppError),
}

async fn fetch_and_extract(
    config: &Config,
    http: &reqwest::Client,
    url: &str,
    cookie_header: Option<&str>,
) -> Result<String, ArticleExtractError> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::USER_AGENT,
        reqwest::header::HeaderValue::from_static("infofetcher/0.1"),
    );
    if let Some(cookie_header) = cookie_header
        && let Ok(value) = reqwest::header::HeaderValue::from_str(cookie_header)
    {
        headers.insert(reqwest::header::COOKIE, value);
    }
    let response = get_with_safe_redirects(http, url, &headers, config.allow_private_fetch)
        .await
        .map_err(ArticleExtractError::from)?;
    let status = response.status();
    if matches!(
        status,
        reqwest::StatusCode::FORBIDDEN
            | reqwest::StatusCode::UNAUTHORIZED
            | reqwest::StatusCode::TOO_MANY_REQUESTS
    ) {
        return Err(ArticleExtractError::Blocked(status));
    }
    if !status.is_success() {
        return Err(AppError::Upstream(format!("正文抓取失败: {status}")).into());
    }
    let bytes = read_response_limited(response, config.article_max_bytes)
        .await
        .map_err(ArticleExtractError::from)?;
    let html = String::from_utf8_lossy(&bytes);
    Ok(extract_text_from_html(&html))
}

async fn browser_fetch_and_extract(
    config: &Config,
    url: &str,
) -> Result<String, ArticleExtractError> {
    if !browser_fetch_url_is_allowed(url) {
        return Err(AppError::BadRequest("浏览器抓取只支持 http/https URL".to_string()).into());
    }
    validate_http_url(url, config.allow_private_fetch).await?;

    let mut command = Command::new(&config.browser_fetch_bin);
    command
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--disable-background-networking",
            "--dump-dom",
        ])
        .arg(format!(
            "--virtual-time-budget={}",
            config.browser_fetch_wait_ms
        ));
    if let Some(user_data_dir) = &config.browser_fetch_user_data_dir {
        command.arg(format!("--user-data-dir={}", user_data_dir.display()));
    }
    command.arg(url).kill_on_drop(true);

    let output = time::timeout(config.browser_fetch_timeout, command.output())
        .await
        .map_err(|_| AppError::Upstream("浏览器抓取超时".to_string()))?
        .map_err(|err| AppError::Upstream(format!("浏览器启动失败: {err}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(
            AppError::Upstream(format!("浏览器抓取失败: {}", clip_message(&stderr, 300))).into(),
        );
    }

    let html = String::from_utf8_lossy(&output.stdout);
    Ok(extract_text_from_html(&html))
}

fn browser_fetch_url_is_allowed(url: &str) -> bool {
    reqwest::Url::parse(url)
        .map(|url| matches!(url.scheme(), "http" | "https"))
        .unwrap_or(false)
}

fn extracted_or_fallback(
    text: String,
    fallback: String,
    fallback_message: &str,
) -> ExtractedContent {
    let text = clean_extracted_text(&text);
    if text.is_empty() {
        ExtractedContent {
            text: fallback,
            error: Some(fallback_message.to_string()),
        }
    } else {
        ExtractedContent { text, error: None }
    }
}

fn browser_text_is_better(direct: &str, browser: &str) -> bool {
    !browser.is_empty() && (direct.is_empty() || browser.chars().count() > direct.chars().count())
}

pub fn extract_text_from_html(html: &str) -> String {
    let document = Html::parse_document(html);
    let selectors = [
        "[itemprop='articleBody']",
        ".t-content__body",
        ".article-body",
        ".article__body",
        ".article-content",
        ".article__content",
        ".story-body",
        ".entry-content",
        ".post-content",
        "article",
        "main",
        "body",
    ]
    .into_iter()
    .filter_map(|selector| Selector::parse(selector).ok())
    .collect::<Vec<_>>();

    for selector in selectors {
        if let Some(element) = document.select(&selector).next() {
            let text = extract_text_from_element(element);
            if text.chars().count() >= 80 {
                return clean_extracted_text(&text);
            }
        }
    }

    clean_extracted_text(&extract_text_from_element(document.root_element()))
}

fn extract_text_from_element(element: ElementRef<'_>) -> String {
    let block_selector = Selector::parse("p, li, blockquote, pre, h1, h2, h3, h4, h5, h6")
        .expect("static block selector must parse");
    let paragraph_selector =
        Selector::parse("p, blockquote, pre").expect("static paragraph selector must parse");
    let mut paragraphs = Vec::new();

    for block in element.select(&block_selector) {
        if block_is_boilerplate(block) {
            continue;
        }
        if block.value().name() == "li" && block.select(&paragraph_selector).next().is_some() {
            continue;
        }
        let text = normalize_whitespace(&block.text().collect::<Vec<_>>().join(" "));
        if !text.is_empty() && paragraphs.last() != Some(&text) {
            paragraphs.push(text);
        }
    }

    if !paragraphs.is_empty() {
        paragraphs.join("\n\n")
    } else {
        let text = element
            .descendants()
            .filter_map(|node| node.value().as_text().map(|text| (node, text)))
            .filter(|(node, _)| {
                !node
                    .ancestors()
                    .filter_map(ElementRef::wrap)
                    .any(element_is_boilerplate)
            })
            .map(|(_, text)| text.as_ref())
            .collect::<Vec<_>>()
            .join(" ");
        normalize_whitespace(&text)
    }
}

fn block_is_boilerplate(block: ElementRef<'_>) -> bool {
    block
        .ancestors()
        .filter_map(ElementRef::wrap)
        .any(element_is_boilerplate)
}

fn element_is_boilerplate(element: ElementRef<'_>) -> bool {
    if matches!(
        element.value().name(),
        "script" | "style" | "noscript" | "template" | "svg" | "nav" | "aside" | "footer" | "form"
    ) {
        return true;
    }
    if matches!(
        element.value().attr("role"),
        Some("navigation" | "complementary" | "contentinfo")
    ) {
        return true;
    }

    ["class", "id"]
        .into_iter()
        .filter_map(|attribute| element.value().attr(attribute))
        .flat_map(|value| value.split(|character: char| !character.is_ascii_alphanumeric()))
        .map(str::to_ascii_lowercase)
        .any(|token| {
            matches!(
                token.as_str(),
                "advertisement"
                    | "comments"
                    | "footer"
                    | "mostread"
                    | "newsletter"
                    | "recommendation"
                    | "recommendations"
                    | "related"
                    | "share"
                    | "sidebar"
                    | "social"
            )
        })
}

/// Removes publisher navigation and recommendation blocks without rewriting article prose.
/// The boundary list is intentionally limited to standalone, unambiguous section headings.
pub fn clean_extracted_text(text: &str) -> String {
    let text = inline_tail_boundary(text)
        .map(|boundary| &text[..boundary])
        .unwrap_or(text);
    let paragraphs = text
        .split("\n\n")
        .map(normalize_whitespace)
        .filter(|paragraph| !paragraph.is_empty())
        .collect::<Vec<_>>();
    if paragraphs.is_empty() {
        return String::new();
    }

    let mut cleaned = Vec::with_capacity(paragraphs.len());
    let mut retained_chars = 0usize;
    for paragraph in paragraphs {
        if retained_chars >= 400 && is_tail_boilerplate_boundary(&paragraph) {
            break;
        }
        if is_discardable_ui_paragraph(&paragraph) {
            continue;
        }
        retained_chars += paragraph.chars().count();
        if cleaned.last() != Some(&paragraph) {
            cleaned.push(paragraph);
        }
    }

    cleaned.join("\n\n")
}

fn inline_tail_boundary(text: &str) -> Option<usize> {
    let lower = text.to_ascii_lowercase();
    [
        " read next ",
        " today's top stories ",
        " today’s top stories ",
        " most read ",
        " you may also be interested in: ",
        " related topics ",
        " more on this story ",
        " watch today's videos ",
        " watch today’s videos ",
    ]
    .into_iter()
    .filter_map(|marker| lower.find(marker))
    .filter(|index| text[..*index].chars().count() >= 200)
    .filter(|index| text[*index..].chars().count() >= 100)
    .min()
}

pub fn clean_article_text(text: &str, title: &str) -> String {
    let cleaned = clean_extracted_text(text);
    let normalized_title = normalize_whitespace(title);
    let mut paragraphs = cleaned
        .split("\n\n")
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if let Some(position) = paragraphs
        .iter()
        .take(8)
        .position(|paragraph| paragraph == &normalized_title)
    {
        let remaining_chars = paragraphs[position + 1..]
            .iter()
            .map(|paragraph| paragraph.chars().count())
            .sum::<usize>();
        if remaining_chars >= 80 {
            paragraphs.drain(..=position);
        }
    }
    paragraphs.join("\n\n")
}

pub fn content_looks_like_markup_artifact(text: &str) -> bool {
    let trimmed = text.trim_start();
    let prefix = trimmed
        .chars()
        .take(500)
        .collect::<String>()
        .to_ascii_lowercase();
    (trimmed.chars().count() >= 500 && !trimmed.contains("\n\n"))
        && (prefix.starts_with(".css-")
            || prefix.starts_with(".a-svg")
            || prefix.starts_with("<!doctype")
            || prefix.starts_with("<html")
            || prefix.starts_with("<div")
            || prefix.starts_with("%pdf-")
            || trimmed.chars().count() >= 50_000
            || prefix.contains("document.createelement(")
            || prefix.contains("background:url('data:image"))
}

pub fn best_article_text(title: &str, content: &str, extracted_content: &str) -> String {
    let preferred = if extracted_content.trim().is_empty()
        || content_looks_like_markup_artifact(extracted_content)
    {
        content
    } else {
        extracted_content
    };
    clean_article_text(preferred, title)
}

fn is_tail_boilerplate_boundary(paragraph: &str) -> bool {
    let marker = paragraph
        .trim()
        .trim_end_matches([':', '：'])
        .replace(['’', '‘'], "'")
        .to_ascii_lowercase();
    matches!(
        marker.as_str(),
        "read next"
            | "today's top stories"
            | "top stories"
            | "most read"
            | "most popular"
            | "you may also be interested in"
            | "related topics"
            | "more on this story"
            | "today's videos"
            | "latest videos"
            | "recommended"
            | "recommended stories"
    ) || marker.starts_with("watch today's videos")
}

fn is_discardable_ui_paragraph(paragraph: &str) -> bool {
    let marker = paragraph
        .trim()
        .trim_end_matches([':', '：'])
        .to_ascii_lowercase();
    matches!(
        marker.as_str(),
        "advertisement" | "skip advertisement" | "sponsor message" | "save" | "share" | "copy link"
    ) || marker.starts_with("click here to share on social media")
        || (marker.starts_with("the post ")
            && marker.ends_with(" appeared first on the github blog ."))
        || marker.starts_with("watch more ")
}

pub fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip_message(value: &str, limit: usize) -> String {
    let trimmed = value.trim();
    let mut clipped = trimmed.chars().take(limit).collect::<String>();
    if trimmed.chars().count() > limit {
        clipped.push_str("...");
    }
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn extracts_article_text_before_body_noise() {
        let html = r#"
            <html><body><nav>menu</nav><article><h1>标题</h1><p>第一段内容。</p><p>第二段内容用于阅读。</p></article></body></html>
        "#;
        let text = extract_text_from_html(html);
        assert_eq!(text, "标题\n\n第一段内容。\n\n第二段内容用于阅读。");
    }

    #[test]
    fn normalizes_whitespace() {
        assert_eq!(normalize_whitespace("a\n\n b\tc"), "a b c");
    }

    #[test]
    fn trims_france24_recommendations_after_article_body() {
        let body = format!(
            "{}\n\n(FRANCE 24 with AP)\n\nRead next\n\nOther story\n\nToday's top stories\n\nAnother story",
            "Reported article paragraph. ".repeat(30)
        );
        let cleaned = clean_extracted_text(&body);
        assert!(cleaned.ends_with("(FRANCE 24 with AP)"));
        assert!(!cleaned.contains("Read next"));
        assert!(!cleaned.contains("Other story"));
    }

    #[test]
    fn trims_common_related_content_boundaries() {
        for boundary in [
            "You may also be interested in:",
            "Related topics",
            "More on this story",
            "Most read",
        ] {
            let body = format!(
                "{}\n\n{boundary}\n\nUnrelated headline",
                "Body. ".repeat(100)
            );
            assert!(!clean_extracted_text(&body).contains("Unrelated headline"));
        }
    }

    #[test]
    fn keeps_short_articles_and_legitimate_section_headings() {
        let body = "Read next\n\nA short note";
        assert_eq!(clean_extracted_text(body), body);

        let body = format!(
            "{}\n\nAnalysis\n\n{}",
            "Intro. ".repeat(80),
            "Detail. ".repeat(20)
        );
        assert!(clean_extracted_text(&body).contains("Analysis"));
    }

    #[test]
    fn ignores_script_style_and_related_html_regions() {
        let html = r#"
          <article>
            <style>.article { color: red; }</style>
            <h1>Title</h1><p>Main paragraph.</p><p>Second paragraph.</p>
            <aside><p>Sidebar headline</p></aside>
            <div class="related-stories"><p>Related headline</p></div>
            <script>window.tracking = true;</script>
          </article>
        "#;
        let text = extract_text_from_html(html);
        assert_eq!(text, "Title\n\nMain paragraph.\n\nSecond paragraph.");
    }

    #[test]
    fn removes_leading_breadcrumbs_and_duplicate_title() {
        let title = "A reported story";
        let body = format!(
            "Home\n\nWorld\n\n{title}\n\n{}",
            "Actual reporting. ".repeat(20)
        );
        let cleaned = clean_article_text(&body, title);
        assert!(cleaned.starts_with("Actual reporting."));
        assert!(!cleaned.contains("Home"));
        assert!(!cleaned.contains(title));
    }

    #[test]
    fn falls_back_from_markup_and_binary_artifacts() {
        let feed = "A concise feed summary with actual reporting.";
        let css = format!(
            ".css-abcd{{background:url('data:image/svg+xml')}}{}",
            "x".repeat(600)
        );
        assert_eq!(best_article_text("Title", feed, &css), feed);
        let pdf = format!("%PDF-1.7 {}", "binary".repeat(100));
        assert_eq!(best_article_text("Title", feed, &pdf), feed);
        let oversized_payload = "video player data ".repeat(3_000);
        assert!(oversized_payload.chars().count() >= 50_000);
        assert_eq!(best_article_text("Title", feed, &oversized_payload), feed);
    }

    #[test]
    fn trims_navigation_compressed_into_one_line() {
        let body = format!(
            "{} Watch Today’s Videos More in Washington Another headline {}",
            "Reported video context. ".repeat(20),
            "Navigation item. ".repeat(30)
        );
        let cleaned = clean_extracted_text(&body);
        assert!(cleaned.ends_with("Reported video context."));
        assert!(!cleaned.contains("Watch Today’s Videos"));
        assert!(!cleaned.contains("Another headline"));
    }

    #[test]
    fn prefers_feed_content_for_v2ex_article_urls() {
        assert!(feed_content_is_preferred(
            "https://www.v2ex.com/t/1208778#reply5"
        ));
        assert!(feed_content_is_preferred("https://v2ex.com/t/1208778"));
        assert!(!feed_content_is_preferred("https://example.com/t/1208778"));
    }

    #[tokio::test]
    async fn v2ex_short_feed_content_does_not_fetch_original_page() {
        let http = reqwest::Client::new();
        let extracted = extract_article_content(
            &test_config(false),
            &http,
            "https://www.v2ex.com/t/1208778#reply5",
            "短正文",
            None,
        )
        .await;

        assert_eq!(extracted.text, "短正文");
        assert_eq!(extracted.error, None);
    }

    #[tokio::test]
    async fn protected_original_page_falls_back_to_feed_content() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 1024];
            let _ = socket.read(&mut buffer).await;
            socket
                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });

        let http = reqwest::Client::new();
        let extracted = extract_article_content(
            &test_config(false),
            &http,
            &format!("http://{addr}/post"),
            "Feed 摘要",
            None,
        )
        .await;

        assert_eq!(extracted.text, "Feed 摘要");
        assert_eq!(
            extracted.error.as_deref(),
            Some("原文站点拒绝自动抓取（403 Forbidden），已显示 Feed 内容")
        );
    }

    #[tokio::test]
    async fn configured_cookie_can_unlock_original_page() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 2048];
            let size = socket.read(&mut buffer).await.unwrap();
            let request = String::from_utf8_lossy(&buffer[..size]);
            if request
                .to_ascii_lowercase()
                .contains("cookie: session=ok; cf_clearance=token")
            {
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<html><body><article>Unlocked original article text.</article></body></html>",
                    )
                    .await
                    .unwrap();
            } else {
                socket
                    .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                    .await
                    .unwrap();
            }
        });

        let http = reqwest::Client::new();
        let extracted = extract_article_content(
            &test_config(false),
            &http,
            &format!("http://{addr}/post"),
            "Feed 摘要",
            Some("session=ok; cf_clearance=token"),
        )
        .await;

        assert_eq!(extracted.text, "Unlocked original article text.");
        assert_eq!(extracted.error, None);
    }

    #[test]
    fn browser_fetch_only_allows_http_urls() {
        assert!(browser_fetch_url_is_allowed("https://example.com/post"));
        assert!(browser_fetch_url_is_allowed("http://example.com/post"));
        assert!(!browser_fetch_url_is_allowed("file:///etc/passwd"));
    }

    #[test]
    fn browser_text_replaces_direct_text_only_when_longer() {
        assert!(browser_text_is_better("", "rendered article"));
        assert!(browser_text_is_better(
            "js required",
            "rendered article body"
        ));
        assert!(!browser_text_is_better("direct article body", "short"));
    }

    fn test_config(browser_fetch_enabled: bool) -> Config {
        Config {
            database_url: String::new(),
            app_bind: "127.0.0.1:8080".parse().unwrap(),
            app_public_url: String::new(),
            admin_username: "admin".to_string(),
            auth_token: "secret".to_string(),
            cookie_secure: false,
            session_days: 14,
            content_retention_days: 7,
            index_dir: ".".into(),
            web_dist_dir: ".".into(),
            fetch_concurrency: 1,
            fetch_interval_minutes: 30,
            feed_max_bytes: 1024 * 1024,
            article_max_bytes: 1024 * 1024,
            osint_searxng_url: None,
            intel_api_token: None,
            embedding_base_url: None,
            embedding_model: String::new(),
            embedding_timeout: std::time::Duration::from_secs(1),
            allow_private_fetch: true,
            llm_base_url: String::new(),
            llm_api_key: String::new(),
            llm_model: String::new(),
            llm_timeout: std::time::Duration::from_secs(1),
            llm_max_concurrency: 1,
            llm_max_output_tokens: 0,
            llm_context_budget_chars: 64_000,
            event_agent_enabled: false,
            event_agent_interval: std::time::Duration::from_secs(30),
            event_agent_lookback_hours: 24,
            event_agent_max_pending: 12,
            event_agent_update_debounce: std::time::Duration::from_secs(30 * 60),
            translation_base_url: "https://api.deepseek.com".to_string(),
            translation_api_key: String::new(),
            translation_model: "deepseek-v4-flash".to_string(),
            translation_timeout: std::time::Duration::from_secs(1),
            translation_max_concurrency: 1,
            translation_max_input_chars: 60_000,
            translation_chunk_chars: 6_000,
            translation_max_output_tokens: 8_192,
            telegram_api_base: "https://api.telegram.org".to_string(),
            telegram_bot_token: String::new(),
            telegram_chat_id: String::new(),
            telegram_timeout: std::time::Duration::from_secs(1),
            opencli_bin: "opencli".to_string(),
            opencli_timeout: std::time::Duration::from_secs(1),
            opencli_max_concurrency: 1,
            opencli_access_token: String::new(),
            rsshub_base_url: None,
            websub_enabled: false,
            websub_lease_seconds: 3600,
            browser_fetch_enabled,
            browser_fetch_bin: "chromium".to_string(),
            browser_fetch_user_data_dir: None,
            browser_fetch_timeout: std::time::Duration::from_secs(1),
            browser_fetch_wait_ms: 1,
            article_fetch_cookies: Default::default(),
            opencli_feeds: Default::default(),
            web_watches: Default::default(),
            editorial: Default::default(),
        }
    }
}
