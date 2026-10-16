//! Bounded research with optional public retrieval and snapshot-bound citations.
use super::*;
use crate::extractor;
use std::collections::{BTreeSet, HashMap};
use tokio::sync::Semaphore;

const MAX_MATERIALS: i64 = 30;
const MAX_TEXT: usize = 12_000;
const MAX_LEADS: usize = 12;
static RESEARCH_SLOTS: Semaphore = Semaphore::const_new(2);
mod evidence;
mod online;

#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ResearchMode {
    Local,
    #[default]
    Online,
}

// Frozen jobs created before online research existed must remain offline.
fn legacy_local_mode() -> ResearchMode {
    ResearchMode::Local
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v2/osint/tools", get(capabilities))
        .route("/api/v2/events/{id}/osint/research", post(research))
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResearchRequest {
    pub(super) query: Option<String>,
    #[serde(default)]
    mode: ResearchMode,
}

#[derive(Serialize, Deserialize)]
struct Lead {
    article_id: Uuid,
    title: String,
    url: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Material {
    id: Uuid,
    title: String,
    url: String,
    canonical_url: String,
    publisher: String,
    reported_published_at: Option<DateTime<Utc>>,
    retrieved_at: DateTime<Utc>,
    text_sha256: String,
    full_text_sha256: String,
    content: String,
    content_truncated: bool,
    dependency_group: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    document: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retrieval: Option<Value>,
}

#[derive(Serialize)]
struct ToolStep {
    tool: &'static str,
    status: &'static str,
    message: String,
}

impl ToolStep {
    fn done(tool: &'static str, message: impl Into<String>) -> Self {
        Self {
            tool,
            status: "done",
            message: message.into(),
        }
    }
}

#[derive(Serialize)]
struct Finding {
    kind: &'static str,
    message: String,
    article_ids: Vec<Uuid>,
}

#[derive(Serialize)]
struct Fragment {
    id: String,
    evidence_ids: Vec<String>,
    kind: &'static str,
    excerpt: String,
    numbers: Vec<String>,
    article_ids: Vec<Uuid>,
    dependency_groups: usize,
}

#[derive(Serialize)]
struct Report {
    format: &'static str,
    run_id: Uuid,
    case_id: Uuid,
    event_id: Uuid,
    event_title: String,
    captures: Vec<Value>,
    query: String,
    generated_at: DateTime<Utc>,
    input_captured_at: DateTime<Utc>,
    status: &'static str,
    total_sources: i64,
    dependency_groups: usize,
    steps: Vec<ToolStep>,
    leads: Vec<Lead>,
    materials: Vec<Material>,
    fragments: Vec<Fragment>,
    findings: Vec<Finding>,
    warnings: Vec<String>,
    mode: ResearchMode,
    evidence: Vec<evidence::Evidence>,
    citation_audit: evidence::CitationAudit,
    research: online::ResearchState,
}

async fn capabilities(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Json<Value>> {
    auth::require_session(&state.pool, &headers).await?;
    Ok(Json(json!({
        "agent": "native-osint-v4", "network_access": true, "default_mode": "online", "model_required": false,
        "search_provider": if state.config.osint_searxng_url.is_some() { "searxng" } else { "gdelt" },
        "tools": ["local_search", "search", "open_page", "find_in_page", "gap_check", "citation_audit", "source_audit", "text_comparison", "timeline_audit", "evidence_fragments", "document_extraction"],
        "casebook": {"persistent": true, "review_history": true, "entity_alias_search": true, "local_documents": true, "durable_jobs": true},
        "limits": {"materials": MAX_MATERIALS, "local_leads": MAX_LEADS, "text_chars": MAX_TEXT, "entity_scan_runs": 20, "documents_per_case":20, "file_bytes":10485760, "pdf_pages":20, "page_chars":30000, "job_seconds":300, "searches":3, "page_attempts":6, "online_seconds":90}
    })))
}

async fn research(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<ResearchRequest>,
) -> AppResult<impl IntoResponse> {
    let actor = auth::require_session(&state.pool, &headers).await?;
    let _slot = RESEARCH_SLOTS
        .try_acquire()
        .map_err(|_| AppError::RateLimited("调查任务已满，请稍后重试".into()))?;
    let mut tx = state.pool.begin().await?;
    let locked: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("osint-research:{id}"))
            .fetch_one(&mut *tx)
            .await?;
    if !locked {
        return Err(AppError::RateLimited("该事件正在调查".into()));
    }
    let mut report =
        tokio::time::timeout(Duration::from_secs(120), investigate(&state, id, request))
            .await
            .map_err(|_| AppError::Upstream("调查超时，请稍后重试".into()))??;
    report.case_id = super::casebook::ensure_case(&mut tx, id, &report.event_title).await?;
    let value = serde_json::to_value(&report).map_err(|e| AppError::Anyhow(e.into()))?;
    // Hash the canonical serde_json Value serialization, not PostgreSQL's textual
    // JSON representation. An export can reproduce this without the live reader.
    let hash = text_hash(&value.to_string());
    sqlx::query(
        "INSERT INTO osint_runs (id, case_id, created_at, created_by, report, report_sha256)
        VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(report.run_id)
    .bind(report.case_id)
    .bind(report.generated_at)
    .bind(actor)
    .bind(value)
    .bind(hash)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(report)))
}

#[derive(Serialize, Deserialize)]
pub(super) struct Input {
    #[serde(default = "legacy_local_mode")]
    mode: ResearchMode,
    id: Uuid,
    title: String,
    query: String,
    materials: Vec<Material>,
    total_sources: i64,
    leads: Vec<Lead>,
    search_available: bool,
    captures: Vec<Value>,
    prepared_at: DateTime<Utc>,
    excluded_documents: i64,
}

pub(super) async fn prepare(
    state: &AppState,
    id: Uuid,
    request: ResearchRequest,
    case_id: Option<Uuid>,
) -> AppResult<Input> {
    let (title, topic): (String, bool) = if let Some(case_id) = case_id {
        sqlx::query_as("SELECT title, origin = 'topic' FROM osint_cases WHERE id = $1")
            .bind(case_id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or(AppError::NotFound)?
    } else {
        let title =
            sqlx::query_scalar("SELECT representative_title FROM story_clusters WHERE id = $1")
                .bind(id)
                .fetch_optional(&state.pool)
                .await?
                .ok_or(AppError::NotFound)?;
        (title, false)
    };
    let query = normalized_query(
        request
            .query
            .as_deref()
            .unwrap_or(&title.chars().take(240).collect::<String>()),
    )?;
    let (mut materials, mut total_sources) = if topic {
        (vec![], 0)
    } else {
        load_materials(state, Some(id), &[]).await?
    };
    let mut excluded_documents = 0;
    if let Some(case_id) = case_id {
        let rows = sqlx::query("SELECT id,filename,sha256,extraction_sha256,extraction,created_at FROM osint_documents WHERE case_id = $1 AND status = 'ready' ORDER BY created_at,id LIMIT 20")
            .bind(case_id).fetch_all(&state.pool).await?;
        total_sources += rows.len() as i64;
        excluded_documents = sqlx::query_scalar(
            "SELECT count(*) FROM osint_documents WHERE case_id = $1 AND status <> 'ready'",
        )
        .bind(case_id)
        .fetch_one(&state.pool)
        .await?;
        for row in rows {
            let extraction: Value = row.get("extraction");
            let text = extraction["pages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|page| {
                    format!(
                        "[第 {} 页]\n{}",
                        page["page"],
                        page["text"].as_str().unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            let content: String = text.chars().take(MAX_TEXT).collect();
            let doc_id: Uuid = row.get("id");
            materials.push(Material {
                id: doc_id, title: row.get("filename"), url: format!("/api/v2/osint/documents/{doc_id}/original"),
                canonical_url: format!("local-document:{doc_id}"), publisher: "local-import".into(),
                reported_published_at: None, retrieved_at: row.get("created_at"), text_sha256: text_hash(&content),
                full_text_sha256: text_hash(&text), content_truncated: content.len()<text.len() || extraction["pages"].as_array().unwrap().iter().any(|p|p["truncated"]==true),
                content, dependency_group:0,
                document:Some(json!({"id":doc_id,"sha256":row.get::<String,_>("sha256"),"extraction_sha256":row.get::<String,_>("extraction_sha256"),"pages":extraction["pages"].as_array().unwrap().iter().map(|p|json!({"page":p["page"],"method":p["method"],"sha256":p["sha256"],"truncated":p["truncated"]})).collect::<Vec<_>>() })),
                retrieval: None,
            });
        }
    }
    let limit = if topic {
        MAX_MATERIALS
    } else {
        MAX_LEADS as i64
    };
    let (mut leads, search_available) = match local_search(state, id, &query, limit).await {
        Ok(leads) => (leads, true),
        Err(_) => (vec![], false),
    };
    if topic {
        // A topic case has no event cluster: local search hits become its base materials.
        let ids: Vec<Uuid> = leads.drain(..).map(|lead| lead.article_id).collect();
        let (found, total) = load_materials(state, None, &ids).await?;
        total_sources += total;
        materials.splice(0..0, found);
    }
    let article_ids: Vec<Uuid> = materials
        .iter()
        .filter(|m| m.document.is_none())
        .map(|m| m.id)
        .collect();
    let captures: Vec<Value> = sqlx::query_scalar("SELECT DISTINCT ON (article_id) jsonb_build_object(
        'id', id, 'article_id', article_id, 'captured_at', captured_at, 'sha256', sha256,
        'byte_length', octet_length(body), 'details', details)
        FROM osint_captures WHERE article_id = ANY($1) ORDER BY article_id, captured_at DESC, id DESC")
        .bind(article_ids).fetch_all(&state.pool).await?;
    Ok(Input {
        mode: request.mode,
        id,
        title,
        query,
        materials,
        total_sources,
        leads,
        search_available,
        captures,
        prepared_at: Utc::now(),
        excluded_documents,
    })
}

async fn investigate(state: &AppState, id: Uuid, request: ResearchRequest) -> AppResult<Report> {
    let case_id: Option<Uuid> = sqlx::query_scalar("SELECT id FROM osint_cases WHERE event_id=$1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?;
    run(state, prepare(state, id, request, case_id).await?).await
}

pub(super) async fn execute(
    state: &AppState,
    input: Value,
    run_id: Uuid,
    case_id: Uuid,
) -> AppResult<Value> {
    let input = serde_json::from_value(input).map_err(|e| AppError::Anyhow(e.into()))?;
    let mut report = run(state, input).await?;
    report.run_id = run_id;
    report.case_id = case_id;
    serde_json::to_value(report).map_err(|e| AppError::Anyhow(e.into()))
}

async fn run(state: &AppState, mut input: Input) -> AppResult<Report> {
    let research = if input.mode == ResearchMode::Online {
        online::collect(&mut input, state.config.osint_searxng_url.as_deref()).await
    } else {
        online::ResearchState::local()
    };
    analyze(input, research).await
}

async fn analyze(input: Input, mut research: online::ResearchState) -> AppResult<Report> {
    let Input {
        mode,
        id,
        title,
        query,
        mut materials,
        total_sources,
        leads,
        search_available,
        captures,
        prepared_at,
        excluded_documents,
    } = input;
    let groups = group_materials(&mut materials);
    let mut steps = vec![
        ToolStep::done(
            "local_evidence",
            format!(
                "冻结 {} 篇正文材料；候选材料共 {total_sources} 篇",
                materials.len()
            ),
        ),
        ToolStep {
            tool: "local_search",
            status: if search_available {
                "done"
            } else {
                "unavailable"
            },
            message: if search_available {
                format!("站内索引找到 {} 条相关线索", leads.len())
            } else {
                "站内索引不可用，不能解释为没有相关报道".into()
            },
        },
    ];
    // CPU work is bounded and moved off the async runtime's executor threads.
    let focus = query.clone();
    let (materials, mut fragments, findings) = tokio::task::spawn_blocking(move || {
        let fragments = extract_focused_fragments(&materials, &focus);
        let findings = audit_materials(&materials, &fragments);
        (materials, fragments, findings)
    })
    .await
    .map_err(|e| AppError::Anyhow(e.into()))?;
    let evidence = evidence::bind(&materials, &mut fragments)?;
    let citation_audit = evidence::validate(&materials, &fragments, &evidence)?;
    research.finish(
        &materials,
        &fragments,
        &findings,
        total_sources,
        excluded_documents,
        search_available,
    );
    steps.extend([
        ToolStep::done(
            "source_audit",
            format!("按发布者、规范链接和完整正文指纹识别 {groups} 个已知依赖组"),
        ),
        ToolStep::done(
            "text_comparison",
            "对正文前 4,000 个字母/数字字符做五字符片段相似度比较，标注疑似转载",
        ),
        ToolStep::done(
            "timeline_audit",
            "检查缺失发布时间及发布时间明显晚于抓取时间的材料",
        ),
        ToolStep::done(
            "evidence_fragments",
            format!(
                "规则抽取 {} 条可定位片段；标注更正措辞和同句数字差异",
                fragments.len()
            ),
        ),
    ]);
    steps.extend([
        ToolStep::done(
            "find_in_page",
            "在正文中优先定位检索词、更正和数字句段，标题及搜索摘要不作为证据",
        ),
        ToolStep::done(
            "gap_check",
            format!(
                "保留 {} 项待核查缺口，停止原因已记录在研究状态中",
                research.gaps.len()
            ),
        ),
        ToolStep::done(
            "citation_audit",
            format!(
                "校验 {} 条引用的 ID、正文哈希与精确位置；未做语义蕴含核验",
                evidence.len()
            ),
        ),
    ]);
    let mut warnings = vec![
        "规则结果用于定位待核对材料，不作语义蕴含判断，也不判定事实真伪。".into(),
        "不同依赖组不等于独立来源；相同文字或不同数字均不能单独证明一致或矛盾。".into(),
        "搜索候选和站内线索不等于正文证据；材料只纳入本次调查快照，不自动加入事件。".into(),
        "调查已保存为独立快照。正文最多 12,000 字符/篇，最多 5 段/篇；复核意见不改写原始快照。"
            .into(),
    ];
    if mode == ResearchMode::Online {
        warnings.push("联网只发送界面中的检索词及固定补查词，不发送本地正文或文件；未调用模型，缺口检查为规则判断。".into());
        if research.provider == "gdelt" {
            warnings.push("当前使用 GDELT 近一个月新闻检索，不代表全网或历史资料覆盖；通用网页检索可配置 SearXNG。".into());
        }
        if research.stop_reason == "provider_unavailable" || research.stop_reason == "time_budget" {
            warnings.push("联网检索未完整完成，已保留成功读取的材料与失败记录。".into());
        }
    }
    if total_sources > materials.len() as i64 {
        warnings
            .push("本次只分析质量排序前 30 篇中的非空正文；空正文和超出预算的材料未分析。".into());
    }
    if steps.iter().any(|s| s.status == "unavailable") {
        warnings.push("站内检索未完成，相关材料覆盖不完整。".into());
    }
    if excluded_documents > 0 {
        warnings.push(format!(
            "排队时有 {excluded_documents} 个文件尚未成功提取，本版未纳入；提取完成后可再次调查。"
        ));
    }
    if materials.iter().any(|m| m.document.is_some()) {
        warnings.push("本地文件按同一未知发布者保守分组，不因上传多个文件增加独立来源数；OCR 和页码标记须对照原件核实。".into());
    }
    Ok(Report {
        format: "rss-osint-research-v4",
        run_id: Uuid::new_v4(),
        case_id: Uuid::nil(),
        event_id: id,
        event_title: title,
        captures,
        query,
        generated_at: Utc::now(),
        input_captured_at: prepared_at,
        status: if groups < 2 {
            "insufficient"
        } else {
            "analyzed"
        },
        total_sources,
        dependency_groups: groups,
        steps,
        leads,
        materials,
        fragments,
        findings,
        warnings,
        mode,
        evidence,
        citation_audit,
        research,
    })
}

pub(super) fn normalized_query(raw: &str) -> AppResult<String> {
    let value = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.is_empty() || value.chars().count() > 240 {
        return Err(AppError::BadRequest("检索词须为 1–240 字符".into()));
    }
    Ok(value)
}

fn literal_search_query(query: &str) -> String {
    // Treat input as words, never as Tantivy operators or arbitrary field expressions.
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .take(12)
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(" OR ")
}

async fn local_search(state: &AppState, id: Uuid, query: &str, limit: i64) -> AppResult<Vec<Lead>> {
    let index = state.search.clone();
    let query = literal_search_query(query);
    if query.is_empty() {
        return Ok(vec![]);
    }
    let ids = tokio::task::spawn_blocking(move || index.search(&query, 60))
        .await
        .map_err(|e| AppError::Anyhow(e.into()))??;
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let rows = sqlx::query(
        "SELECT a.id, a.title, a.url FROM articles a
        LEFT JOIN source_feedback sf ON sf.feed_id = a.feed_id
        WHERE a.id = ANY($1) AND a.story_id IS DISTINCT FROM $2 AND NOT COALESCE(sf.muted, false)
        ORDER BY array_position($1::uuid[], a.id) LIMIT $3",
    )
    .bind(&ids)
    .bind(id)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Lead {
            article_id: r.get("id"),
            title: r.get("title"),
            url: r.get("url"),
        })
        .collect())
}

async fn load_materials(
    state: &AppState,
    story: Option<Uuid>,
    ids: &[Uuid],
) -> AppResult<(Vec<Material>, i64)> {
    let rows = sqlx::query("SELECT a.id, a.title, a.url, a.canonical_url, a.content, a.extracted_content,
        a.published_at, a.fetched_at, f.site_url, f.feed_url, f.hub_adapter, f.hub_path, count(*) OVER () AS total
        FROM articles a JOIN feeds f ON f.id = a.feed_id
        LEFT JOIN source_feedback sf ON sf.feed_id = f.id
        WHERE (a.story_id = $1 OR a.id = ANY($3)) AND NOT COALESCE(sf.muted, false)
        ORDER BY a.quality_score DESC, a.fetched_at DESC, a.id LIMIT $2")
        .bind(story).bind(MAX_MATERIALS).bind(ids).fetch_all(&state.pool).await?;
    let total = rows.first().map(|r| r.get("total")).unwrap_or(0);
    let materials = rows
        .into_iter()
        .filter_map(|row| {
            let title: String = row.get("title");
            let text = extractor::best_article_text(
                &title,
                &row.get::<String, _>("content"),
                &row.get::<String, _>("extracted_content"),
            );
            let text = extractor::normalize_whitespace(&text);
            if text.is_empty() {
                return None;
            }
            let content: String = text.chars().take(MAX_TEXT).collect();
            Some(Material {
                id: row.get("id"),
                title,
                url: row.get("url"),
                canonical_url: row.get("canonical_url"),
                publisher: publisher_family(
                    row.get::<Option<String>, _>("site_url").as_deref(),
                    &row.get::<String, _>("feed_url"),
                    row.get::<Option<String>, _>("hub_adapter").as_deref(),
                    row.get::<Option<String>, _>("hub_path").as_deref(),
                ),
                reported_published_at: row.get("published_at"),
                retrieved_at: row.get("fetched_at"),
                text_sha256: text_hash(&content),
                full_text_sha256: text_hash(&text),
                content_truncated: content.len() < text.len(),
                content,
                dependency_group: 0,
                document: None,
                retrieval: None,
            })
        })
        .collect();
    Ok((materials, total))
}

fn text_hash(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn group_materials(materials: &mut [Material]) -> usize {
    let mut sources: Vec<_> = materials
        .iter()
        .map(|m| Source {
            article_id: m.id,
            title: m.title.clone(),
            source_title: m.publisher.clone(),
            publisher: m.publisher.clone(),
            url: m.url.clone(),
            canonical_url: canonicalize_url(&m.canonical_url),
            content_hash: m.full_text_sha256.clone(),
            reported_published_at: m.reported_published_at,
            fetched_at: m.retrieved_at,
            dependency_group: 0,
            latest_capture: None,
        })
        .collect();
    let groups = group_sources(&mut sources);
    for (material, source) in materials.iter_mut().zip(sources) {
        material.dependency_group = source.dependency_group;
    }
    groups
}

fn sentences(content: &str) -> Vec<&str> {
    let mut start = 0;
    let mut out = Vec::new();
    for (i, c) in content.char_indices() {
        let end = i + c.len_utf8();
        if "。！？!?；;".contains(c)
            || (c == '.'
                && content[end..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace))
        {
            out.push(content[start..end].trim());
            start = end;
        }
    }
    if start < content.len() {
        out.push(content[start..].trim());
    }
    out
}

fn numbers(text: &str) -> Vec<String> {
    let chars: Vec<_> = text.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < chars.len()
            && (chars[i].is_ascii_digit()
                || (matches!(chars[i], '.' | ',')
                    && chars.get(i + 1).is_some_and(char::is_ascii_digit)))
        {
            i += 1;
        }
        out.push(chars[start..i].iter().collect());
    }
    out
}

fn is_correction(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "更正",
        "澄清",
        "撤回",
        "勘误",
        "辟谣",
        "correction",
        "clarification",
        "retraction",
        "corrected",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

#[cfg(test)]
fn extract_fragments(materials: &[Material]) -> Vec<Fragment> {
    extract_focused_fragments(materials, "")
}

fn extract_focused_fragments(materials: &[Material], query: &str) -> Vec<Fragment> {
    let mut fragments: Vec<Fragment> = vec![];
    let mut positions = HashMap::<String, usize>::new();
    for material in materials {
        let mut candidates: Vec<_> = sentences(&material.content)
            .into_iter()
            .filter(|s| (16..=360).contains(&s.chars().count()))
            .collect();
        candidates.sort_by_key(|s| {
            if is_correction(s) {
                0
            } else if online::matches_query(s, query) {
                1
            } else if !numbers(s).is_empty() {
                2
            } else {
                3
            }
        });
        let mut seen = HashSet::new();
        for excerpt in candidates
            .into_iter()
            .filter(|s| seen.insert(*s))
            .take(if query.is_empty() { 3 } else { 5 })
        {
            if let Some(&i) = positions.get(excerpt) {
                fragments[i].article_ids.push(material.id);
            } else {
                positions.insert(excerpt.to_string(), fragments.len());
                let numbers = numbers(excerpt);
                fragments.push(Fragment {
                    id: format!("F-{}", text_hash(excerpt)),
                    evidence_ids: vec![],
                    kind: if is_correction(excerpt) {
                        "correction"
                    } else if numbers.is_empty() {
                        "statement"
                    } else {
                        "numeric"
                    },
                    excerpt: excerpt.to_string(),
                    numbers,
                    article_ids: vec![material.id],
                    dependency_groups: 0,
                });
            }
        }
    }
    for fragment in &mut fragments {
        fragment.dependency_groups = materials
            .iter()
            .filter(|m| fragment.article_ids.contains(&m.id))
            .map(|m| m.dependency_group)
            .collect::<HashSet<_>>()
            .len();
    }
    fragments
}

fn number_template(text: &str) -> String {
    let mut template = text.to_lowercase();
    let mut values = numbers(text);
    values.sort_by_key(|v| std::cmp::Reverse(v.len()));
    for value in values {
        template = template.replace(&value, "#");
    }
    template
}

fn shingles(content: &str) -> HashSet<String> {
    let chars: Vec<_> = content
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .take(4_000)
        .collect();
    chars.windows(5).map(|w| w.iter().collect()).collect()
}

fn audit_materials(materials: &[Material], fragments: &[Fragment]) -> Vec<Finding> {
    audit_materials_inner(materials, fragments, true)
}

fn audit_materials_inner(
    materials: &[Material],
    fragments: &[Fragment],
    compare_text: bool,
) -> Vec<Finding> {
    let mut findings = vec![];
    let groups = materials
        .iter()
        .map(|m| m.dependency_group)
        .collect::<HashSet<_>>()
        .len();
    if groups < 2 {
        findings.push(Finding {
            kind: "coverage",
            message: "不足两个已知依赖组，需补充独立原始材料。".into(),
            article_ids: materials.iter().map(|m| m.id).collect(),
        });
    }
    let missing: Vec<_> = materials
        .iter()
        .filter(|m| m.reported_published_at.is_none())
        .map(|m| m.id)
        .collect();
    if !missing.is_empty() {
        findings.push(Finding {
            kind: "missing_time",
            message: "这些材料缺少源报发布时间；抓取时间不能替代事件发生时间。".into(),
            article_ids: missing,
        });
    }
    let future: Vec<_> = materials
        .iter()
        .filter(|m| {
            m.reported_published_at
                .is_some_and(|t| t > m.retrieved_at + chrono::Duration::hours(24))
        })
        .map(|m| m.id)
        .collect();
    if !future.is_empty() {
        findings.push(Finding {
            kind: "time_anomaly",
            message: "源报发布时间比抓取时间晚超过 24 小时，请核对时区、预告日期或元数据。".into(),
            article_ids: future,
        });
    }
    // Pairwise reprint comparison is only needed for the final report, not
    // every online gap check.
    let shingled: Vec<_> = if compare_text {
        materials.iter().map(|m| shingles(&m.content)).collect()
    } else {
        vec![]
    };
    for i in 0..materials.len() {
        if !compare_text {
            break;
        }
        for j in i + 1..materials.len() {
            if materials[i].dependency_group == materials[j].dependency_group {
                continue;
            }
            let a = &shingled[i];
            let b = &shingled[j];
            if a.len() < 80 || b.len() < 80 {
                continue;
            }
            let common = a.intersection(b).count();
            let union = a.len() + b.len() - common;
            if common * 100 >= union * 85 {
                findings.push(Finding {
                    kind: "possible_reprint",
                    message: format!(
                        "正文片段相似度约 {}%，可能转载同一材料；未据此自动认定同源。",
                        common * 100 / union
                    ),
                    article_ids: vec![materials[i].id, materials[j].id],
                });
            }
        }
    }
    let mut numeric: HashMap<String, Vec<&Fragment>> = HashMap::new();
    for fragment in fragments {
        if fragment.kind == "correction" {
            findings.push(Finding {
                kind: "correction_cue",
                message: "存在更正/澄清措辞，请打开对应片段核对对象与时间。".into(),
                article_ids: fragment.article_ids.clone(),
            });
        }
        if !fragment.numbers.is_empty() {
            numeric
                .entry(number_template(&fragment.excerpt))
                .or_default()
                .push(fragment);
        }
    }
    // Stable output for exported reports and repeatable review.
    let mut templates: Vec<_> = numeric.into_iter().collect();
    templates.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, rows) in templates {
        let values: HashSet<_> = rows.iter().map(|r| &r.numbers).collect();
        let ids: BTreeSet<_> = rows
            .iter()
            .flat_map(|r| r.article_ids.iter().copied())
            .collect();
        if values.len() > 1 && ids.len() > 1 {
            findings.push(Finding {
                kind: "numeric_difference",
                message: "相同句式出现不同数字，需核对统计口径、单位和时间；也可能是正常更新。"
                    .into(),
                article_ids: ids.into_iter().collect(),
            });
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn material(publisher: &str, content: &str) -> Material {
        Material {
            id: Uuid::new_v4(),
            title: publisher.into(),
            url: format!("https://{publisher}.test/a"),
            canonical_url: format!("https://{publisher}.test/a"),
            publisher: publisher.into(),
            reported_published_at: None,
            retrieved_at: Utc::now(),
            text_sha256: text_hash(content),
            full_text_sha256: text_hash(content),
            content: content.into(),
            content_truncated: false,
            dependency_group: 0,
            document: None,
            retrieval: None,
        }
    }
    #[test]
    fn full_fingerprint_and_transitive_groups_preserve_independence_boundary() {
        let mut items = vec![
            material("a", "Same report"),
            material("b", "Same report"),
            material("c", "Distinct report"),
        ];
        assert_eq!(group_materials(&mut items), 2);
        items[1].full_text_sha256 = text_hash("Different ending after the same displayed prefix");
        assert_eq!(group_materials(&mut items), 3);
        items[1].canonical_url = items[0].canonical_url.clone();
        assert_eq!(group_materials(&mut items), 2);
    }
    #[test]
    fn numeric_differences_are_cues_and_excerpts_remain_verbatim() {
        let mut items = vec![
            material(
                "a",
                "The agency reported 12.5 percent growth in the survey.",
            ),
            material(
                "b",
                "The agency reported 15.5 percent growth in the survey.",
            ),
        ];
        group_materials(&mut items);
        let fragments = extract_fragments(&items);
        assert_eq!(fragments[0].numbers, vec!["12.5"]);
        assert_eq!(fragments.len(), 2);
        assert!(
            fragments
                .iter()
                .all(|f| items.iter().any(|m| m.content.contains(&f.excerpt)))
        );
        let findings = audit_materials(&items, &fragments);
        assert!(findings.iter().any(|f| f.kind == "numeric_difference"));
        items[1].content =
            "The other agency reported 15.5 percent growth in a different survey.".into();
        assert!(
            !audit_materials(&items, &extract_fragments(&items))
                .iter()
                .any(|f| f.kind == "numeric_difference")
        );
    }
    #[test]
    fn chinese_corrections_and_dates_never_become_verified_facts() {
        let mut items = vec![
            material("a", "更正：此前公布的检测样本数量为120份，实际应为125份。"),
            material("b", "第二家媒体尚未公开原始检测记录与样本统计细节。"),
        ];
        items[1].reported_published_at = Some(items[1].retrieved_at + chrono::Duration::days(2));
        group_materials(&mut items);
        let fragments = extract_fragments(&items);
        assert_eq!(fragments[0].kind, "correction");
        let findings = audit_materials(&items, &fragments);
        for kind in ["correction_cue", "missing_time", "time_anomaly"] {
            assert!(findings.iter().any(|f| f.kind == kind));
        }
    }
    #[test]
    fn syndicated_fragments_do_not_multiply_support_groups() {
        let text = "The agency published the full investigation records today.";
        let mut items = vec![material("a", text), material("b", text)];
        group_materials(&mut items);
        let fragments = extract_fragments(&items);
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0].article_ids.len(), 2);
        assert_eq!(fragments[0].dependency_groups, 1);
    }
    #[test]
    fn similar_text_is_flagged_without_merging_groups() {
        let text: String = (0..100)
            .map(|i| format!("source observation {i} with measurements and location details. "))
            .collect();
        let mut items = vec![
            material("a", &text),
            material("b", &format!("{text} Added context.")),
        ];
        assert_eq!(group_materials(&mut items), 2);
        assert!(
            audit_materials(&items, &extract_fragments(&items))
                .iter()
                .any(|f| f.kind == "possible_reprint")
        );
    }
    #[test]
    fn query_is_literal_and_legacy_external_options_are_rejected() {
        assert_eq!(
            literal_search_query("title:news OR (record*)"),
            "\"title\" OR \"news\" OR \"OR\" OR \"record\""
        );
        for body in [
            json!({"provider":"gdelt"}),
            json!({"provider":"searxng"}),
            json!({"analyze":true}),
        ] {
            assert!(serde_json::from_value::<ResearchRequest>(body).is_err());
        }
        assert!(normalized_query(&"字".repeat(241)).is_err());
        assert!(normalized_query("  ").is_err());
    }
    #[tokio::test]
    #[ignore = "requires disposable migrated RSS_OSINT_TEST_DATABASE_URL; no external network"]
    async fn research_roundtrip_with_postgres() {
        use crate::{config::Config, models::SearchDocument};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use tokio::sync::mpsc;
        let pool = sqlx::PgPool::connect(&std::env::var("RSS_OSINT_TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        let mut config = (*Config::from_env().unwrap()).clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let trap = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        config.llm_base_url = format!("http://{}", trap.local_addr().unwrap());
        config.llm_api_key = Uuid::new_v4().to_string();
        let trap_server = tokio::spawn(async move {
            axum::serve(
                trap,
                Router::new().fallback(move || {
                    counter.fetch_add(1, Ordering::SeqCst);
                    async { axum::http::StatusCode::BAD_GATEWAY }
                }),
            )
            .await
            .unwrap();
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
        let mut documents = vec![];
        for (host, text) in [
            (
                "alpha.example",
                "Acme opened an office in Paris. First publisher original record. ",
            ),
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
            let article: Uuid = sqlx::query_scalar("INSERT INTO articles (feed_id, guid, url, url_hash, title, canonical_url, content_hash, story_id, content)
                VALUES ($1, $2, $3, $2, 'research fixture', $3, $2, $4, $5) RETURNING id")
                .bind(feed).bind(Uuid::new_v4().to_string()).bind(format!("https://{host}/article"))
                .bind(story).bind(text.repeat(40)).fetch_one(&pool).await.unwrap();
            documents.push(SearchDocument {
                id: article,
                feed_title: host.into(),
                title: "research fixture".into(),
                content: text.repeat(40),
                url: format!("https://{host}/article"),
                published_at: None,
            });
        }
        let related: Uuid = sqlx::query_scalar("INSERT INTO articles (feed_id, guid, url, url_hash, title, content, canonical_url, content_hash)
            VALUES ($1, $2, 'https://alpha.example/related', $2, 'research fixture background', 'Other record', 'https://alpha.example/related', $2) RETURNING id")
            .bind(feeds[0]).bind(Uuid::new_v4().to_string()).fetch_one(&pool).await.unwrap();
        documents.push(SearchDocument {
            id: related,
            feed_title: "alpha".into(),
            title: "research fixture background".into(),
            content: "Other record".into(),
            url: "https://alpha.example/related".into(),
            published_at: None,
        });
        documents.push(SearchDocument {
            id: Uuid::new_v4(),
            feed_title: "stale".into(),
            title: "research fixture deleted article".into(),
            content: "Gone".into(),
            url: "https://stale.example/".into(),
            published_at: None,
        });
        state.search.index_articles(&documents).unwrap();
        let token = Uuid::new_v4().to_string();
        let token_hash = hex::encode(Sha256::digest(token.as_bytes()));
        sqlx::query("INSERT INTO sessions (token_hash, username, expires_at) VALUES ($1, 'fixture', now() + interval '1 hour')")
            .bind(&token_hash).execute(&pool).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let worker_state = state.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, crate::osint::router().with_state(state))
                .await
                .unwrap();
        });
        let http = reqwest::Client::new();
        let cookie = format!("rss_session={token}");
        let path = format!("{base}/api/v2/events/{story}/osint/research");
        assert_eq!(
            http.post(&path)
                .json(&json!({"mode":"local"}))
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
            .json(&json!({"mode":"local"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let report: Value = response.json().await.unwrap();
        assert_eq!(report["format"], "rss-osint-research-v4");
        assert_eq!(report["status"], "analyzed");
        assert_eq!(report["materials"].as_array().unwrap().len(), 2);
        assert_eq!(report["dependency_groups"], 2);
        assert_eq!(report["leads"].as_array().unwrap().len(), 1);
        assert_eq!(report["leads"][0]["article_id"], related.to_string());
        assert!(!report["fragments"].as_array().unwrap().is_empty());
        assert!(
            report["materials"]
                .as_array()
                .unwrap()
                .iter()
                .all(|m| m["reported_published_at"].is_null())
        );
        super::super::casebook_tests::exercise(
            &pool,
            &http,
            &base,
            &cookie,
            &report,
            documents[0].id,
        )
        .await;
        for body in [
            json!({"provider":"gdelt"}),
            json!({"provider":"searxng"}),
            json!({"analyze":true}),
        ] {
            assert_eq!(
                http.post(&path)
                    .header(header::COOKIE, &cookie)
                    .json(&body)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                422
            );
        }
        let tools: Value = http
            .get(format!("{base}/api/v2/osint/tools"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(tools["network_access"], true);
        assert_eq!(tools["default_mode"], "online");
        assert_eq!(tools["model_required"], false);
        let capture_path = format!(
            "{base}/api/v2/events/{story}/evidence/{}/capture",
            documents[0].id
        );
        assert_eq!(
            http.post(capture_path)
                .header(header::COOKIE, &cookie)
                .json(&json!({"include_archive":true}))
                .send()
                .await
                .unwrap()
                .status(),
            400
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
                .json(&json!({"mode":"local"}))
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
            .json(&json!({"mode":"local"}))
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
        sqlx::query("INSERT INTO source_feedback (feed_id, muted) VALUES ($1, true)")
            .bind(feeds[0])
            .execute(&pool)
            .await
            .unwrap();
        let empty: Value = http
            .post(&path)
            .header(header::COOKIE, &cookie)
            .json(&json!({"mode":"local"}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(empty["materials"].as_array().unwrap().len(), 0);
        assert_eq!(empty["leads"].as_array().unwrap().len(), 0);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "native investigation must not call a model"
        );
        super::super::jobs_tests::exercise(&worker_state, &http, &base, &cookie).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "background jobs must not call a model"
        );
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
        trap_server.abort();
        server.abort();
        pool.close().await;
    }
}
