//! Native OSINT workflow. No network clients, model calls or external tools.
use super::*;
use crate::extractor;
use std::collections::{BTreeSet, HashMap};
use tokio::sync::Semaphore;

const MAX_MATERIALS: i64 = 30;
const MAX_TEXT: usize = 12_000;
const MAX_LEADS: usize = 12;
static RESEARCH_SLOTS: Semaphore = Semaphore::const_new(2);

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v2/osint/tools", get(capabilities))
        .route("/api/v2/events/{id}/osint/research", post(research))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResearchRequest {
    query: Option<String>,
}

#[derive(Serialize)]
struct Lead {
    article_id: Uuid,
    title: String,
    url: String,
}

#[derive(Clone, Serialize)]
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
    kind: &'static str,
    excerpt: String,
    numbers: Vec<String>,
    article_ids: Vec<Uuid>,
    dependency_groups: usize,
}

#[derive(Serialize)]
struct Report {
    format: &'static str,
    event_id: Uuid,
    query: String,
    generated_at: DateTime<Utc>,
    status: &'static str,
    total_sources: i64,
    dependency_groups: usize,
    steps: Vec<ToolStep>,
    leads: Vec<Lead>,
    materials: Vec<Material>,
    fragments: Vec<Fragment>,
    findings: Vec<Finding>,
    warnings: Vec<String>,
}

async fn capabilities(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Json<Value>> {
    auth::require_session(&state.pool, &headers).await?;
    Ok(Json(json!({
        "agent": "native-osint-v2", "network_access": false, "model_required": false,
        "tools": ["local_search", "source_audit", "text_comparison", "timeline_audit", "evidence_fragments"],
        "limits": {"materials": MAX_MATERIALS, "local_leads": MAX_LEADS, "text_chars": MAX_TEXT}
    })))
}

async fn research(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<ResearchRequest>,
) -> AppResult<impl IntoResponse> {
    auth::require_session(&state.pool, &headers).await?;
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
    let report = tokio::time::timeout(Duration::from_secs(20), investigate(&state, id, request))
        .await
        .map_err(|_| AppError::Upstream("本地调查超时，请稍后重试".into()))??;
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
    let query = normalized_query(
        request
            .query
            .as_deref()
            .unwrap_or(&title.chars().take(240).collect::<String>()),
    )?;
    let (mut materials, total_sources) = load_materials(state, id).await?;
    let groups = group_materials(&mut materials);
    let mut steps = vec![ToolStep::done(
        "local_evidence",
        format!(
            "读取 {} 篇有正文材料；事件共有 {total_sources} 篇未静音材料",
            materials.len()
        ),
    )];
    let leads = match local_search(state, id, &query).await {
        Ok(leads) => {
            steps.push(ToolStep::done(
                "local_search",
                format!("站内索引找到 {} 条其他事件/未归类材料线索", leads.len()),
            ));
            leads
        }
        Err(_) => {
            steps.push(ToolStep {
                tool: "local_search",
                status: "unavailable",
                message: "站内索引查询失败；不能解释为没有相关报道".into(),
            });
            vec![]
        }
    };
    // CPU work is bounded and moved off the async runtime's executor threads.
    let (materials, fragments, findings) = tokio::task::spawn_blocking(move || {
        let fragments = extract_fragments(&materials);
        let findings = audit_materials(&materials, &fragments);
        (materials, fragments, findings)
    })
    .await
    .map_err(|e| AppError::Anyhow(e.into()))?;
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
    let mut warnings = vec![
        "规则结果用于定位待核对材料，不作语义蕴含判断，也不判定事实真伪。".into(),
        "不同依赖组不等于独立来源；相同文字或不同数字均不能单独证明一致或矛盾。".into(),
        "调查只读取现有数据库与站内索引，不查询互联网或模型；相关线索不自动加入事件。".into(),
        "报告只保留在本页，刷新前请导出。正文导出最多 12,000 字符/篇，数字线索最多 3 段/篇。"
            .into(),
    ];
    if total_sources > materials.len() as i64 {
        warnings
            .push("本次只分析质量排序前 30 篇中的非空正文；空正文和超出预算的材料未分析。".into());
    }
    if steps.iter().any(|s| s.status == "unavailable") {
        warnings.push("站内检索未完成，相关材料覆盖不完整。".into());
    }
    Ok(Report {
        format: "rss-osint-research-v2",
        event_id: id,
        query,
        generated_at: Utc::now(),
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
    })
}

fn normalized_query(raw: &str) -> AppResult<String> {
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

async fn local_search(state: &AppState, id: Uuid, query: &str) -> AppResult<Vec<Lead>> {
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
    .bind(MAX_LEADS as i64)
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

async fn load_materials(state: &AppState, id: Uuid) -> AppResult<(Vec<Material>, i64)> {
    let rows = sqlx::query("SELECT a.id, a.title, a.url, a.canonical_url, a.content, a.extracted_content,
        a.published_at, a.fetched_at, f.site_url, f.feed_url, f.hub_adapter, f.hub_path, count(*) OVER () AS total
        FROM articles a JOIN feeds f ON f.id = a.feed_id
        LEFT JOIN source_feedback sf ON sf.feed_id = f.id
        WHERE a.story_id = $1 AND NOT COALESCE(sf.muted, false)
        ORDER BY a.quality_score DESC, a.fetched_at DESC, a.id LIMIT $2")
        .bind(id).bind(MAX_MATERIALS).fetch_all(&state.pool).await?;
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

fn extract_fragments(materials: &[Material]) -> Vec<Fragment> {
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
            } else if !numbers(s).is_empty() {
                1
            } else {
                2
            }
        });
        let mut seen = HashSet::new();
        for excerpt in candidates.into_iter().filter(|s| seen.insert(*s)).take(3) {
            if let Some(&i) = positions.get(excerpt) {
                fragments[i].article_ids.push(material.id);
            } else {
                positions.insert(excerpt.to_string(), fragments.len());
                let numbers = numbers(excerpt);
                fragments.push(Fragment {
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
    let shingled: Vec<_> = materials.iter().map(|m| shingles(&m.content)).collect();
    for i in 0..materials.len() {
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
    fn material(publisher: &str, content: &str) -> Material {
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
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let report: Value = response.json().await.unwrap();
        assert_eq!(report["format"], "rss-osint-research-v2");
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
        assert_eq!(tools["network_access"], false);
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
        sqlx::query("INSERT INTO source_feedback (feed_id, muted) VALUES ($1, true)")
            .bind(feeds[0])
            .execute(&pool)
            .await
            .unwrap();
        let empty: Value = http
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
        assert_eq!(empty["materials"].as_array().unwrap().len(), 0);
        assert_eq!(empty["leads"].as_array().unwrap().len(), 0);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "native investigation must not call a model"
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
