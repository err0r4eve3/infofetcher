//! Bounded, on-demand investigation. Search results are leads, never evidence.
use super::*;
use crate::{
    config::Config,
    extractor,
    llm::{self, GeneratedSummary, SummarySource},
};
use tokio::sync::Semaphore;

const MAX_MATERIALS: usize = 5;
const MAX_HITS: usize = 12;
const MAX_FETCHES: usize = 4;
const MAX_TEXT: usize = 12_000;
static RESEARCH_SLOTS: Semaphore = Semaphore::const_new(2);

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v2/osint/tools", get(capabilities))
        .route("/api/v2/events/{id}/osint/research", post(research))
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Provider {
    #[default]
    Local,
    Gdelt,
    Searxng,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResearchRequest {
    #[serde(default)]
    provider: Provider,
    query: Option<String>,
    #[serde(default)]
    analyze: bool,
}

#[derive(Clone, Debug, Serialize)]
struct Lead {
    title: String,
    url: String,
}

#[derive(Clone, Serialize)]
struct Material {
    id: Uuid,
    origin: &'static str,
    title: String,
    url: String,
    publisher: String,
    reported_published_at: Option<DateTime<Utc>>,
    retrieved_at: DateTime<Utc>,
    text_sha256: String,
    content: String,
    dependency_group: usize,
    selected: bool,
}

#[derive(Serialize)]
struct ToolStep {
    tool: &'static str,
    status: &'static str,
    message: String,
}

impl ToolStep {
    fn new(tool: &'static str, status: &'static str, message: impl Into<String>) -> Self {
        Self {
            tool,
            status,
            message: message.into(),
        }
    }
}

#[derive(Serialize)]
struct Report {
    format: &'static str,
    event_id: Uuid,
    query: String,
    provider: Provider,
    generated_at: DateTime<Utc>,
    status: &'static str,
    steps: Vec<ToolStep>,
    leads: Vec<Lead>,
    materials: Vec<Material>,
    analysis: Option<GeneratedSummary>,
    core_support_groups: usize,
    warnings: Vec<String>,
}

async fn capabilities(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Json<Value>> {
    auth::require_session(&state.pool, &headers).await?;
    Ok(Json(json!({
        "agent": "bounded-osint-v1",
        "analysis_available": !state.config.llm_api_key.trim().is_empty(),
        "providers": [
            {"id": "local", "name": "现有材料", "configured": true},
            {"id": "gdelt", "name": "GDELT", "configured": true},
            {"id": "searxng", "name": "SearXNG", "configured": state.config.osint_searxng_url.is_some()}
        ],
        "limits": {"search_results": MAX_HITS, "page_fetches": MAX_FETCHES,
            "analysis_sources": MAX_MATERIALS, "timeout_seconds": 180}
    })))
}

async fn research(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<ResearchRequest>,
) -> AppResult<impl IntoResponse> {
    auth::require_session(&state.pool, &headers).await?;
    if request.provider == Provider::Searxng && state.config.osint_searxng_url.is_none() {
        return Err(AppError::BadRequest("尚未配置 SearXNG".into()));
    }
    let _slot = RESEARCH_SLOTS
        .try_acquire()
        .map_err(|_| AppError::RateLimited("调查任务已满，请稍后重试".into()))?;
    // Transaction-scoped lock releases on success, failure, timeout or cancellation.
    let mut tx = state.pool.begin().await?;
    let locked: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("osint-research:{id}"))
            .fetch_one(&mut *tx)
            .await?;
    if !locked {
        return Err(AppError::RateLimited("该事件正在调查".into()));
    }
    let report = tokio::time::timeout(Duration::from_secs(180), investigate(&state, id, request))
        .await
        .map_err(|_| AppError::Upstream("调查超过 180 秒，已停止；未发布分析结果".into()))??;
    tx.rollback().await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(report)))
}

async fn investigate(state: &AppState, id: Uuid, request: ResearchRequest) -> AppResult<Report> {
    let title: String =
        sqlx::query_scalar("SELECT representative_title FROM story_clusters WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or(AppError::NotFound)?;
    let default_query: String = title.chars().take(240).collect();
    let query = normalized_query(request.query.as_deref().unwrap_or(&default_query))?;
    let mut report = Report {
        format: "rss-osint-research-v1",
        event_id: id,
        query: query.clone(),
        provider: request.provider,
        generated_at: Utc::now(),
        status: "evidence_only",
        steps: vec![],
        leads: vec![],
        materials: load_materials(state, id).await?,
        analysis: None,
        core_support_groups: 0,
        warnings: vec![
            "检索结果仅是线索；引文核验仅确认材料支持该陈述，不证明事实真实。".into(),
            "依赖分组只识别已知同源关系；不同组仍可能转述同一声明。".into(),
            "调查为本次快照，不会自动入库、修改事件快讯或推送。长期保存请导出。".into(),
        ],
    };
    report.steps.push(ToolStep::new(
        "local_evidence",
        "done",
        format!(
            "读取 {} 篇有正文的本地材料（最多 30 篇）",
            report.materials.len()
        ),
    ));
    if request.provider != Provider::Local {
        // A deterministic two-perspective plan avoids an additional planner model call.
        let queries = search_plan(&query, request.provider);
        for search_query in queries {
            match search(&state.config, request.provider, &search_query).await {
                Ok(leads) => {
                    report.steps.push(ToolStep::new(
                        "search",
                        "done",
                        format!("{search_query}：返回 {} 条线索", leads.len()),
                    ));
                    // Interleave perspectives so corrections are not crowded out by the first query.
                    let old = std::mem::take(&mut report.leads);
                    report.leads = interleave_leads(old, leads);
                }
                Err(_) => report.steps.push(ToolStep::new(
                    "search",
                    "unavailable",
                    format!(
                        "{search_query}：检索失败，可能为超时、限流或非 JSON 响应；不能视为没有报道"
                    ),
                )),
            }
        }
        let local_urls: HashSet<_> = report
            .materials
            .iter()
            .map(|m| canonicalize_url(&m.url))
            .collect();
        let candidates: Vec<_> = report
            .leads
            .iter()
            .filter(|lead| !local_urls.contains(&canonicalize_url(&lead.url)))
            .take(MAX_FETCHES)
            .cloned()
            .collect();
        for lead in candidates {
            match read_page(&lead).await {
                Ok(material) => {
                    report.steps.push(ToolStep::new(
                        "read_page",
                        "done",
                        format!("已读取原文：{}", lead.title),
                    ));
                    report.materials.push(material);
                }
                Err(_) => report.steps.push(ToolStep::new(
                    "read_page",
                    "unavailable",
                    format!("原文不可用或正文不足：{}；未把检索标题当作证据", lead.title),
                )),
            }
        }
    }
    select_materials(&mut report.materials);
    let selected: Vec<_> = report
        .materials
        .iter()
        .filter(|m| m.selected)
        .map(|m| SummarySource {
            article_id: m.id,
            source_title: m.publisher.clone(),
            article_title: m.title.clone(),
            url: m.url.clone(),
            content: m.content.clone(),
        })
        .collect();
    report.steps.push(ToolStep::new(
        "source_audit",
        "done",
        format!(
            "按发布者、规范 URL、正文指纹分组后选取 {} 篇材料",
            selected.len()
        ),
    ));
    if selected.len() < 2 {
        report.status = "insufficient";
        report
            .warnings
            .push("不足两个依赖组，无法形成交叉核验分析。".into());
    } else if !request.analyze {
        report.steps.push(ToolStep::new(
            "claim_verifier",
            "skipped",
            "本次仅整理证据，未调用模型",
        ));
    } else if state.config.llm_api_key.trim().is_empty() {
        report.steps.push(ToolStep::new(
            "claim_verifier",
            "unavailable",
            "未配置分析模型，保留证据清单",
        ));
    } else if let Ok(_permit) = state.llm_semaphore.try_acquire() {
        let mut config = (*state.config).clone();
        config.llm_context_budget_chars = config.llm_context_budget_chars.min(40_000);
        config.llm_max_output_tokens = config.llm_max_output_tokens.min(2_048);
        match llm::summarize_research_sources(&config, &state.http, &query, selected).await {
            Ok(analysis) if analysis.method.starts_with("grounded-") => {
                report.core_support_groups = support_groups(&report.materials, &analysis.citations);
                report.status = "grounded";
                report.steps.push(ToolStep::new(
                    "claim_verifier",
                    "done",
                    "完成逐条引文定位与主张支持性核验",
                ));
                report.analysis = Some(analysis);
                if report.core_support_groups < 2 {
                    report
                        .warnings
                        .push("核心主张仍缺少两个依赖组的引文支持；不能视为独立证实。".into());
                }
            }
            Ok(_) => report.steps.push(ToolStep::new(
                "claim_verifier",
                "insufficient",
                "模型核验未通过，未输出抽取摘要作为调查结论",
            )),
            Err(_) => report.steps.push(ToolStep::new(
                "claim_verifier",
                "unavailable",
                "模型请求失败，证据清单仍可导出",
            )),
        }
    } else {
        report.steps.push(ToolStep::new(
            "claim_verifier",
            "unavailable",
            "分析模型繁忙，证据清单仍可导出",
        ));
    }
    if report.steps.iter().any(|step| step.status == "unavailable") {
        report
            .warnings
            .push("部分工具不可用，本次调查范围不完整；见调查过程。".into());
    }
    report.generated_at = Utc::now();
    Ok(report)
}

fn normalized_query(raw: &str) -> AppResult<String> {
    let value = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if !(2..=240).contains(&value.chars().count()) {
        return Err(AppError::BadRequest("检索词须为 2–240 字符".into()));
    }
    Ok(value)
}

fn search_plan(query: &str, provider: Provider) -> Vec<String> {
    match provider {
        Provider::Local => vec![],
        // GDELT accepts its own query language. Preserve user-supplied operators.
        Provider::Gdelt => vec![query.to_string()],
        Provider::Searxng => vec![
            query.to_string(),
            format!("{query} (correction OR clarification OR 更正 OR 澄清)"),
        ],
    }
}

async fn load_materials(state: &AppState, id: Uuid) -> AppResult<Vec<Material>> {
    let rows = sqlx::query(
        "SELECT a.id, a.title, a.url, a.content, a.extracted_content,
        a.published_at, a.fetched_at, f.site_url, f.feed_url, f.hub_adapter, f.hub_path
        FROM articles a JOIN feeds f ON f.id = a.feed_id
        LEFT JOIN source_feedback sf ON sf.feed_id = f.id
        WHERE a.story_id = $1 AND NOT COALESCE(sf.muted, false)
        ORDER BY a.quality_score DESC, a.fetched_at DESC, a.id LIMIT 30",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let title: String = row.get("title");
            let text = extractor::best_article_text(
                &title,
                &row.get::<String, _>("content"),
                &row.get::<String, _>("extracted_content"),
            );
            let content = bounded_text(&text);
            if content.chars().count() < 80 {
                return None;
            }
            Some(Material {
                id: row.get("id"),
                origin: "local",
                title,
                url: row.get("url"),
                publisher: publisher_family(
                    row.get::<Option<String>, _>("site_url").as_deref(),
                    &row.get::<String, _>("feed_url"),
                    row.get::<Option<String>, _>("hub_adapter").as_deref(),
                    row.get::<Option<String>, _>("hub_path").as_deref(),
                ),
                reported_published_at: row.get("published_at"),
                retrieved_at: row.get("fetched_at"),
                text_sha256: text_hash(&content),
                content,
                dependency_group: 0,
                selected: false,
            })
        })
        .collect())
}

fn bounded_text(text: &str) -> String {
    extractor::normalize_whitespace(text)
        .chars()
        .take(MAX_TEXT)
        .collect()
}

fn text_hash(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn material_sources(materials: &[Material]) -> Vec<Source> {
    materials
        .iter()
        .map(|m| Source {
            article_id: m.id,
            title: m.title.clone(),
            source_title: m.publisher.clone(),
            publisher: m.publisher.clone(),
            url: m.url.clone(),
            canonical_url: canonicalize_url(&m.url),
            content_hash: m.text_sha256.clone(),
            reported_published_at: m.reported_published_at,
            fetched_at: m.retrieved_at,
            dependency_group: 0,
            latest_capture: None,
        })
        .collect()
}

fn select_materials(materials: &mut [Material]) {
    let mut sources = material_sources(materials);
    group_sources(&mut sources);
    for (material, source) in materials.iter_mut().zip(&sources) {
        material.dependency_group = source.dependency_group;
        material.selected = false;
    }
    let mut selected_groups = HashSet::new();
    // Alternate local and discovered evidence so both sides reach the verifier's five-source budget.
    let locals: Vec<_> = materials
        .iter()
        .enumerate()
        .filter(|(_, m)| m.origin == "local")
        .map(|(i, _)| i)
        .collect();
    let external: Vec<_> = materials
        .iter()
        .enumerate()
        .filter(|(_, m)| m.origin != "local")
        .map(|(i, _)| i)
        .collect();
    for i in (0..locals.len().max(external.len()))
        .flat_map(|i| [locals.get(i).copied(), external.get(i).copied()])
        .flatten()
    {
        if selected_groups.len() == MAX_MATERIALS {
            break;
        }
        if selected_groups.insert(materials[i].dependency_group) {
            materials[i].selected = true;
        }
    }
}

fn support_groups(materials: &[Material], citations: &[GeneratedCitation]) -> usize {
    citations
        .iter()
        .filter(|c| c.verified && c.claim_id == "summary")
        .filter_map(|c| {
            materials
                .iter()
                .find(|m| m.id == c.article_id && m.selected)
        })
        .map(|m| m.dependency_group)
        .collect::<HashSet<_>>()
        .len()
}

fn search_url(config: &Config, provider: Provider, query: &str) -> AppResult<Url> {
    let mut url = match provider {
        Provider::Gdelt => Url::parse("https://api.gdeltproject.org/api/v2/doc/doc").unwrap(),
        Provider::Searxng => Url::parse(&format!(
            "{}/search",
            config
                .osint_searxng_url
                .as_deref()
                .ok_or_else(|| AppError::BadRequest("未配置 SearXNG".into()))?
                .trim_end_matches('/')
        ))
        .map_err(|_| AppError::BadRequest("SearXNG 地址无效".into()))?,
        Provider::Local => return Err(AppError::BadRequest("本地模式不执行检索".into())),
    };
    match provider {
        Provider::Gdelt => {
            url.query_pairs_mut().extend_pairs([
                ("query", query),
                ("mode", "artlist"),
                ("format", "json"),
                ("maxrecords", "12"),
                ("timespan", "1month"),
                ("sort", "hybridrel"),
            ]);
        }
        Provider::Searxng => {
            url.query_pairs_mut()
                .extend_pairs([("q", query), ("format", "json")]);
        }
        Provider::Local => unreachable!(),
    }
    Ok(url)
}

async fn search(config: &Config, provider: Provider, query: &str) -> AppResult<Vec<Lead>> {
    let url = search_url(config, provider, query)?;
    tokio::time::timeout(Duration::from_secs(20), async {
        // Only the operator-configured SearXNG origin may be private. Never follow its redirects.
        let response = if provider == Provider::Searxng {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .build()?
                .get(url)
                .header(header::USER_AGENT, "rss-hub-reader/0.1 (OSINT search)")
                .send()
                .await?
        } else {
            public_get(url).await?
        };
        if !response.status().is_success() {
            return Err(AppError::Upstream("检索服务不可用".into()));
        }
        let bytes = net::read_response_limited(response, MAX_BYTES).await?;
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| AppError::Upstream("检索返回非 JSON 响应".into()))?;
        parse_leads(&value, provider)
    })
    .await
    .map_err(|_| AppError::Upstream("检索超时".into()))?
}

fn parse_leads(value: &Value, provider: Provider) -> AppResult<Vec<Lead>> {
    let key = if provider == Provider::Gdelt {
        "articles"
    } else {
        "results"
    };
    let rows = value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::Upstream("检索响应缺少结果数组".into()))?;
    let mut seen = HashSet::new();
    Ok(rows
        .iter()
        .filter_map(|row| {
            let url = public_url(row.get("url")?.as_str()?).ok()?;
            let title = row
                .get("title")?
                .as_str()?
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if title.is_empty() || !seen.insert(canonicalize_url(url.as_str())) {
                return None;
            }
            Some(Lead {
                title: title.chars().take(300).collect(),
                url: url.to_string(),
            })
        })
        .take(MAX_HITS)
        .collect())
}

fn interleave_leads(first: Vec<Lead>, second: Vec<Lead>) -> Vec<Lead> {
    let mut seen = HashSet::new();
    (0..first.len().max(second.len()))
        .flat_map(|i| [first.get(i), second.get(i)])
        .flatten()
        .filter(|lead| seen.insert(canonicalize_url(&lead.url)))
        .take(MAX_HITS)
        .cloned()
        .collect()
}

fn public_url(raw: &str) -> AppResult<Url> {
    let mut url = checked_url(raw)?;
    if !matches!(url.port_or_known_default(), Some(80 | 443)) {
        return Err(AppError::BadRequest("仅允许标准 HTTP(S) 端口".into()));
    }
    if let Some(ip) = url
        .host_str()
        .and_then(|h| h.trim_matches(['[', ']']).parse().ok())
        && net::ip_is_private_or_reserved(ip)
    {
        return Err(AppError::BadRequest("禁止访问私有地址".into()));
    }
    if url
        .host_str()
        .is_some_and(|h| h.eq_ignore_ascii_case("localhost") || h.ends_with(".local"))
    {
        return Err(AppError::BadRequest("禁止访问本机地址".into()));
    }
    url.set_fragment(None);
    Ok(url)
}

async fn public_get(mut url: Url) -> AppResult<reqwest::Response> {
    for hop in 0..=5 {
        url = public_url(url.as_str())?;
        let host = url
            .host_str()
            .ok_or_else(|| AppError::BadRequest("缺少主机名".into()))?;
        let addresses = tokio::net::lookup_host((
            host.trim_matches(['[', ']']),
            url.port_or_known_default().unwrap(),
        ))
        .await
        .map_err(|_| AppError::Upstream("公开地址解析失败".into()))?
        .collect::<Vec<_>>();
        if addresses.is_empty()
            || addresses
                .iter()
                .any(|a| net::ip_is_private_or_reserved(a.ip()))
        {
            return Err(AppError::BadRequest("非公开解析地址".into()));
        }
        // Pin the checked addresses, disable proxy environment and redirects: no second DNS lookup.
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(host, &addresses)
            .timeout(Duration::from_secs(15))
            .build()?;
        let response = http
            .get(url.clone())
            .header(header::USER_AGENT, "rss-hub-reader/0.1 (OSINT research)")
            .send()
            .await?;
        if !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            return Ok(response);
        }
        if hop == 5 {
            break;
        }
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| AppError::Upstream("重定向缺少地址".into()))?;
        url = url
            .join(location)
            .map_err(|_| AppError::Upstream("重定向地址无效".into()))?;
    }
    Err(AppError::Upstream("重定向次数过多".into()))
}

async fn read_page(lead: &Lead) -> AppResult<Material> {
    tokio::time::timeout(Duration::from_secs(20), async {
        let response = public_get(public_url(&lead.url)?).await?;
        let url = response.url().to_string();
        if !response.status().is_success() {
            return Err(AppError::Upstream("原文 HTTP 错误".into()));
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let body = net::read_response_limited(response, MAX_BYTES).await?;
        page_material(&body, &content_type, &url)
    })
    .await
    .map_err(|_| AppError::Upstream("读取原文超时".into()))?
}

fn page_material(body: &[u8], content_type: &str, url: &str) -> AppResult<Material> {
    let base = public_url(url)?;
    let text = String::from_utf8_lossy(body);
    let (title, content, published_at) =
        if matches!(content_type, "text/html" | "application/xhtml+xml") {
            let doc = Html::parse_document(&text);
            let title = ["h1", "title"]
                .iter()
                .find_map(|selector| {
                    doc.select(&Selector::parse(selector).unwrap())
                        .next()
                        .map(|node| node.text().collect::<Vec<_>>().join(" "))
                })
                .unwrap_or_else(|| url.to_string());
            let metadata = page_metadata(body, &base);
            let published = metadata["declared_published_at"]
                .as_str()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.with_timezone(&Utc));
            (title, extractor::extract_text_from_html(&text), published)
        } else if content_type == "text/plain" {
            (url.to_string(), text.into_owned(), None)
        } else {
            return Err(AppError::Upstream("暂不支持此原文格式".into()));
        };
    let content = bounded_text(&content);
    if content.chars().count() < 200 {
        return Err(AppError::Upstream("原文正文不足".into()));
    }
    Ok(Material {
        id: Uuid::new_v4(),
        origin: "web",
        title: title
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(300)
            .collect(),
        publisher: publisher_family(Some(url), url, None, None),
        url: url.to_string(),
        reported_published_at: published_at,
        retrieved_at: Utc::now(),
        text_sha256: text_hash(&content),
        content,
        dependency_group: 0,
        selected: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_results_are_bounded_untrusted_leads() {
        let value = json!({"articles": [
            {"title":"Report", "url":"https://example.com/a"},
            {"title":"Mirror", "url":"https://example.com/a?utm_source=rss"},
            {"title":"Private", "url":"http://127.0.0.1/"},
            {"title":"Credentials", "url":"https://user:secret@example.com/"},
            {"title":"File", "url":"file:///etc/passwd"},
            {"title":"Port", "url":"https://example.com:8080/"}
        ]});
        assert_eq!(parse_leads(&value, Provider::Gdelt).unwrap().len(), 1);
        assert!(parse_leads(&json!({"error":"rate limit"}), Provider::Gdelt).is_err());
        assert!(
            parse_leads(&json!({"results":[]}), Provider::Searxng)
                .unwrap()
                .is_empty()
        );
        for raw in [
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://169.254.169.254/",
            "http://localhost/",
            "http://host.local/",
        ] {
            assert!(public_url(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn uses_page_title_not_search_title_and_never_invents_dates() {
        let body = format!(
            "<html><title>Original title</title><article><p>{}</p></article></html>",
            "Public source text. ".repeat(40)
        );
        let material =
            page_material(body.as_bytes(), "text/html", "https://example.com/a").unwrap();
        assert_eq!(material.title, "Original title");
        assert_eq!(material.reported_published_at, None);
        assert_eq!(material.text_sha256, text_hash(&material.content));
        assert!(page_material(b"blocked", "text/html", "https://example.com/a").is_err());
        assert!(
            page_material(body.as_bytes(), "application/pdf", "https://example.com/a").is_err()
        );
    }

    #[test]
    fn syndication_does_not_count_as_corroboration() {
        let material = |id: &str, origin| Material {
            id: Uuid::new_v4(),
            origin,
            title: id.into(),
            url: format!("https://{id}.com/a"),
            publisher: id.into(),
            reported_published_at: None,
            retrieved_at: Utc::now(),
            text_sha256: id.into(),
            content: id.into(),
            dependency_group: 0,
            selected: false,
        };
        let mut materials = vec![
            material("a", "local"),
            material("b", "local"),
            material("c", "web"),
            material("d", "web"),
        ];
        materials[2].text_sha256 = materials[0].text_sha256.clone();
        select_materials(&mut materials);
        assert_eq!(materials[0].dependency_group, materials[2].dependency_group);
        assert_eq!(materials.iter().filter(|m| m.selected).count(), 3);
        let citation = |index: usize, claim: &str| GeneratedCitation {
            claim_id: claim.into(),
            article_id: materials[index].id,
            evidence_id: "E1".into(),
            source_title: "source".into(),
            article_title: "title".into(),
            url: "https://example.com/".into(),
            excerpt: "text".into(),
            verified: true,
        };
        assert_eq!(
            support_groups(
                &materials,
                &[
                    citation(0, "summary"),
                    citation(2, "summary"),
                    citation(1, "bullet:0")
                ]
            ),
            1
        );
    }

    #[test]
    fn correction_leads_survive_the_budget_and_queries_are_bounded() {
        let lead = |n| Lead {
            title: format!("{n}"),
            url: format!("https://example.com/{n}"),
        };
        let combined = interleave_leads((0..12).map(lead).collect(), vec![lead(99), lead(0)]);
        assert_eq!(combined.len(), MAX_HITS);
        assert_eq!(combined[1].title, "99");
        assert!(normalized_query("x").is_err());
        assert!(normalized_query(&"字".repeat(241)).is_err());
        assert_eq!(search_plan("climate", Provider::Searxng).len(), 2);
        assert_eq!(search_plan("climate", Provider::Gdelt).len(), 1);
    }

    #[tokio::test]
    async fn refuses_private_pages_before_requesting() {
        assert!(
            public_get(Url::parse("http://127.0.0.1/").unwrap())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    #[ignore = "requires disposable migrated RSS_OSINT_TEST_DATABASE_URL; no external network"]
    async fn research_roundtrip_with_postgres() {
        use std::sync::Arc;
        use tokio::sync::mpsc;
        let pool = sqlx::PgPool::connect(&std::env::var("RSS_OSINT_TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        let mut config = (*Config::from_env().unwrap()).clone();
        config.llm_api_key.clear();
        let search_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        config.osint_searxng_url =
            Some(format!("http://{}", search_listener.local_addr().unwrap()));
        let search_server = tokio::spawn(async move {
            axum::serve(search_listener, Router::new().route("/search", get(
                |axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>| async move {
                    assert_eq!(query.get("format").unwrap(), "json");
                    assert!(query.get("q").unwrap().contains("research fixture"));
                    Json(json!({"results": [{"title":"Private trap", "url":"http://127.0.0.1/"}]}))
                }
            ))).await.unwrap();
        });
        let index = tempfile::tempdir().unwrap();
        let (worker_tx, _) = mpsc::channel(1);
        let state = AppState {
            config: Arc::new(config),
            pool: pool.clone(),
            http: reqwest::Client::new(),
            search: Arc::new(crate::search::SearchIndex::open(index.path()).unwrap()),
            worker_tx,
            login_limiter: Arc::new(crate::security::LoginRateLimiter::default()),
            opencli_semaphore: Arc::new(Semaphore::new(1)),
            llm_semaphore: Arc::new(Semaphore::new(1)),
            translation_semaphore: Arc::new(Semaphore::new(1)),
        };
        let story: Uuid = sqlx::query_scalar("INSERT INTO story_clusters (representative_title, story_key) VALUES ('research fixture', $1) RETURNING id")
            .bind(Uuid::new_v4().to_string()).fetch_one(&pool).await.unwrap();
        let mut feeds = vec![];
        for (host, text) in [
            ("alpha.example", "First publisher original record. "),
            ("beta.example", "Second publisher distinct observation. "),
        ] {
            let feed: Uuid = sqlx::query_scalar(
                "INSERT INTO feeds (title, feed_url, site_url) VALUES ($1, $2, $2) RETURNING id",
            )
            .bind(host)
            .bind(format!("https://{host}/{}", Uuid::new_v4()))
            .fetch_one(&pool)
            .await
            .unwrap();
            feeds.push(feed);
            sqlx::query("INSERT INTO articles (feed_id, guid, url, url_hash, title, canonical_url, content_hash, story_id, content)
                VALUES ($1, $2, $3, $2, 'research fixture', $3, $2, $4, $5)")
                .bind(feed).bind(Uuid::new_v4().to_string()).bind(format!("https://{host}/article"))
                .bind(story).bind(text.repeat(40)).execute(&pool).await.unwrap();
        }
        let token = Uuid::new_v4().to_string();
        let token_hash = hex::encode(Sha256::digest(token.as_bytes()));
        sqlx::query("INSERT INTO sessions (token_hash, username, expires_at) VALUES ($1, 'fixture', now() + interval '1 hour')")
            .bind(&token_hash).execute(&pool).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router().with_state(state))
                .await
                .unwrap();
        });
        let http = reqwest::Client::new();
        let cookie = format!("rss_session={token}");
        let path = format!("{base}/api/v2/events/{story}/osint/research");
        assert_eq!(
            http.post(&path)
                .json(&json!({}))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        assert_eq!(
            http.get(format!("{base}/api/v2/osint/tools"))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        assert_eq!(
            http.post(&path)
                .header(header::COOKIE, &cookie)
                .json(&json!({"provider":"shell"}))
                .send()
                .await
                .unwrap()
                .status(),
            422
        );
        let response = http
            .post(&path)
            .header(header::COOKIE, &cookie)
            .json(&json!({"analyze":true}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let report: Value = response.json().await.unwrap();
        assert_eq!(report["status"], "evidence_only");
        assert!(report["analysis"].is_null());
        assert_eq!(report["materials"].as_array().unwrap().len(), 2);
        assert!(
            report["materials"]
                .as_array()
                .unwrap()
                .iter()
                .all(|m| m["selected"] == true && m["reported_published_at"].is_null())
        );
        assert!(
            report["steps"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["tool"] == "claim_verifier" && s["status"] == "unavailable")
        );
        let report: Value = http
            .post(&path)
            .header(header::COOKIE, &cookie)
            .json(&json!({"provider":"searxng"}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(report["leads"].as_array().unwrap().len(), 0);
        assert_eq!(
            report["steps"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|s| s["tool"] == "search" && s["status"] == "done")
                .count(),
            2
        );
        // A concurrent request must not perform the investigation while its event lock is held.
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("osint-research:{story}"))
            .execute(&mut *tx)
            .await
            .unwrap();
        assert_eq!(
            http.post(&path)
                .header(header::COOKIE, &cookie)
                .json(&json!({}))
                .send()
                .await
                .unwrap()
                .status(),
            429
        );
        tx.rollback().await.unwrap();
        sqlx::query("INSERT INTO source_feedback (feed_id, muted) VALUES ($1, true)")
            .bind(feeds[1])
            .execute(&pool)
            .await
            .unwrap();
        let report: Value = http
            .post(&path)
            .header(header::COOKIE, &cookie)
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(report["status"], "insufficient");
        assert_eq!(report["materials"].as_array().unwrap().len(), 1);
        for feed in feeds {
            sqlx::query("DELETE FROM feeds WHERE id = $1")
                .bind(feed)
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("DELETE FROM story_clusters WHERE id = $1")
            .bind(story)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash)
            .execute(&pool)
            .await
            .unwrap();
        search_server.abort();
        server.abort();
        pool.close().await;
    }
}
