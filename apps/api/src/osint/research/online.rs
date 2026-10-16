use super::*;
use async_trait::async_trait;
use tokio::time::{Instant, timeout_at};

const SEARCH_LIMIT: usize = 3;
const PAGE_LIMIT: usize = 6;
const CANDIDATE_LIMIT: usize = 24;
const ONLINE_SECONDS: u64 = 90;

#[derive(Clone, Serialize)]
pub(super) struct Gap {
    kind: &'static str,
    message: String,
}

#[derive(Serialize)]
pub(super) struct SearchRound {
    query: String,
    reason: String,
    status: &'static str,
    candidates_added: usize,
    pages_added: usize,
    gaps_after: Vec<Gap>,
}

#[derive(Serialize)]
pub(super) struct Candidate {
    url: String,
    title: String,
    query: String,
    status: &'static str,
    message: String,
    material_id: Option<Uuid>,
    evidence_ids: Vec<String>,
}

#[derive(Serialize)]
pub(super) struct ResearchState {
    pub provider: &'static str,
    pub stop_reason: &'static str,
    pub gaps: Vec<Gap>,
    rounds: Vec<SearchRound>,
    candidates: Vec<Candidate>,
    budget: Budget,
}

#[derive(Serialize)]
struct Budget {
    searches_used: usize,
    searches_limit: usize,
    pages_attempted: usize,
    pages_limit: usize,
    pages_read: usize,
    seconds_limit: u64,
}

impl ResearchState {
    pub(super) fn local() -> Self {
        Self {
            provider: "none",
            stop_reason: "local_only",
            gaps: vec![],
            rounds: vec![],
            candidates: vec![],
            budget: Budget {
                searches_used: 0,
                searches_limit: 0,
                pages_attempted: 0,
                pages_limit: 0,
                pages_read: 0,
                seconds_limit: 0,
            },
        }
    }

    pub(super) fn finish(
        &mut self,
        materials: &[Material],
        fragments: &[Fragment],
        findings: &[Finding],
        total: i64,
        excluded: i64,
        search_available: bool,
    ) {
        self.gaps = review_gaps(materials, fragments, findings);
        if total > materials.len() as i64 || materials.iter().any(|m| m.content_truncated) {
            self.gaps.push(gap(
                "bounded_input",
                "部分正文为空、截断或超过材料预算，尚未覆盖。",
            ));
        }
        if excluded > 0 {
            self.gaps.push(gap(
                "pending_documents",
                "有文件未完成提取，尚未纳入本次调查。",
            ));
        }
        if !search_available {
            self.gaps.push(gap(
                "local_search_unavailable",
                "站内检索失败，不能据此判断无相关材料。",
            ));
        }
        if self.provider != "none" {
            if self.budget.pages_read == 0 {
                self.gaps.push(gap(
                    "no_web_content",
                    "没有成功读取站外正文，联网候选不能作为证据。",
                ));
            }
            if self.candidates.iter().any(|c| c.status == "failed") {
                self.gaps
                    .push(gap("unread_pages", "部分候选页面读取失败，详见来源记录。"));
            }
            if self.rounds.iter().any(|r| r.status != "done") {
                self.gaps.push(gap(
                    "search_failed",
                    "有搜索请求失败或超时，站外覆盖不完整。",
                ));
            }
            if self.stop_reason != "coverage_target" {
                self.gaps.push(gap(
                    "incomplete_search",
                    "检索因预算、无新增材料或服务不可用停止，仍需补查。",
                ));
            }
        }
        for candidate in &mut self.candidates {
            if let Some(id) = candidate.material_id {
                candidate.evidence_ids = fragments
                    .iter()
                    .flat_map(|f| f.article_ids.iter().zip(&f.evidence_ids))
                    .filter(|(source, _)| **source == id)
                    .map(|(_, evidence)| evidence.clone())
                    .collect();
            }
        }
        self.gaps.push(gap(
            "semantic_review",
            "规则只检查文字与来源关系；问题是否回答、引文是否支持结论仍需人工核查。",
        ));
    }
}

fn gap(kind: &'static str, message: &str) -> Gap {
    Gap {
        kind,
        message: message.into(),
    }
}

fn review_gaps(materials: &[Material], fragments: &[Fragment], findings: &[Finding]) -> Vec<Gap> {
    let mut gaps = vec![];
    if materials
        .iter()
        .map(|m| m.dependency_group)
        .collect::<HashSet<_>>()
        .len()
        < 2
    {
        gaps.push(gap(
            "source_coverage",
            "不足两个已知依赖组，需补充不同来源的原始材料。",
        ));
    }
    if fragments.is_empty() {
        gaps.push(gap("no_excerpts", "没有可定位的完整正文句段。"));
    }
    for (kind, message) in [
        (
            "numeric_difference",
            "相同句式出现不同数字，需核对原始统计口径、单位和时间。",
        ),
        (
            "correction_cue",
            "存在更正或澄清措辞，需核对更正对象及版本。",
        ),
        ("time_anomaly", "发布时间存在异常，需要核对日期元数据。"),
        ("missing_time", "部分材料缺少发布时间，不能确定报道时序。"),
    ] {
        if findings.iter().any(|f| f.kind == kind) {
            gaps.push(gap(kind, message));
        }
    }
    gaps
}

pub(super) fn matches_query(text: &str, query: &str) -> bool {
    let lower = text.to_lowercase();
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.chars().count() >= 2)
        .filter(|s| {
            ![
                "the", "and", "for", "with", "what", "how", "does", "this", "that", "from",
            ]
            .contains(&s.to_lowercase().as_str())
        })
        .take(12)
        .any(|term| {
            let term = term.to_lowercase();
            // English word boundaries avoid matching e.g. 'war' inside 'award'.
            lower.match_indices(&term).any(|(start, _)| {
                !term.is_ascii()
                    || (lower[..start]
                        .chars()
                        .next_back()
                        .is_none_or(|c| !c.is_alphanumeric())
                        && lower[start + term.len()..]
                            .chars()
                            .next()
                            .is_none_or(|c| !c.is_alphanumeric()))
            })
        })
}

struct SearchHit {
    url: String,
    title: String,
}

#[async_trait]
trait ResearchTools: Sync {
    async fn search(&self, query: &str) -> AppResult<Vec<SearchHit>>;
    async fn open_page(&self, url: &str) -> AppResult<Material>;
}

struct PublicTools {
    searxng: Option<String>,
}

pub(super) async fn collect(input: &mut Input, searxng: Option<&str>) -> ResearchState {
    let tools = PublicTools {
        searxng: searxng.map(str::to_owned),
    };
    collect_with(
        input,
        &tools,
        if searxng.is_some() {
            "searxng"
        } else {
            "gdelt"
        },
        Duration::from_secs(ONLINE_SECONDS),
    )
    .await
}

async fn collect_with(
    input: &mut Input,
    tools: &impl ResearchTools,
    provider: &'static str,
    duration: Duration,
) -> ResearchState {
    let mut state = ResearchState::local();
    state.provider = provider;
    state.stop_reason = "search_budget";
    state.budget.searches_limit = SEARCH_LIMIT;
    state.budget.pages_limit = PAGE_LIMIT;
    state.budget.seconds_limit = duration.as_secs();
    let deadline = Instant::now() + duration;
    let mut seen: HashSet<String> = input
        .materials
        .iter()
        .map(|m| canonicalize_url(&m.url))
        .collect();
    let mut queries = HashSet::new();
    for round in 0..SEARCH_LIMIT {
        group_materials(&mut input.materials);
        let fragments = extract_focused_fragments(&input.materials, &input.query);
        let findings = audit_materials_inner(&input.materials, &fragments, false);
        let conflict = findings
            .iter()
            .any(|f| matches!(f.kind, "numeric_difference" | "correction_cue"));
        // Never derive external queries from local bodies, uploaded documents or untrusted page instructions.
        let (query, reason) = if round == 0 {
            (input.query.clone(), "发现公开来源")
        } else if conflict {
            (
                format!(
                    "{} {}",
                    input.query,
                    if input.query.is_ascii() {
                        "correction clarification"
                    } else {
                        "更正 澄清"
                    }
                ),
                "核对数字差异或更正线索",
            )
        } else {
            (
                format!(
                    "{} {}",
                    input.query,
                    if round == 1 {
                        "official report"
                    } else {
                        "original source"
                    }
                ),
                "补充原始来源与正文覆盖",
            )
        };
        if !queries.insert(query.clone()) {
            state.stop_reason = "no_progress";
            break;
        }
        let mut record = SearchRound {
            query: query.clone(),
            reason: reason.into(),
            status: "done",
            candidates_added: 0,
            pages_added: 0,
            gaps_after: vec![],
        };
        state.budget.searches_used += 1;
        let result = timeout_at(
            deadline.min(Instant::now() + Duration::from_secs(20)),
            tools.search(&query),
        )
        .await;
        let hits = match result {
            Ok(Ok(hits)) => hits,
            error => {
                record.status = if error.is_err() {
                    "timeout"
                } else {
                    "unavailable"
                };
                // Rate limits and provider failures terminate; do not amplify an outage with retries.
                record.reason = match error {
                    Ok(Err(AppError::RateLimited(_))) => "搜索服务限流；本轮不自动重试".into(),
                    _ => "搜索服务不可用、响应无效或超时；已保留现有材料".into(),
                };
                state.rounds.push(record);
                state.stop_reason = if Instant::now() >= deadline {
                    "time_budget"
                } else {
                    "provider_unavailable"
                };
                break;
            }
        };
        for hit in hits {
            if state.candidates.len() >= CANDIDATE_LIMIT {
                break;
            }
            if checked_url(&hit.url).is_err() || !seen.insert(canonicalize_url(&hit.url)) {
                continue;
            }
            record.candidates_added += 1;
            state.candidates.push(Candidate {
                url: hit.url,
                title: hit.title,
                query: query.clone(),
                status: "discovered",
                message: String::new(),
                material_id: None,
                evidence_ids: vec![],
            });
        }
        // Prefer unread publishers; failed attempts consume budget just like successful reads.
        let mut pending: Vec<usize> = state
            .candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| c.status == "discovered")
            .map(|(i, _)| i)
            .collect();
        pending.sort_by_key(|&i| {
            let c = &state.candidates[i];
            let publisher = publisher_family(Some(&c.url), &c.url, None, None);
            (
                input.materials.iter().any(|m| m.publisher == publisher),
                !matches_query(&c.title, &input.query),
            )
        });
        for index in pending.into_iter().take(2) {
            if state.budget.pages_attempted >= PAGE_LIMIT {
                break;
            }
            if Instant::now() >= deadline {
                state.stop_reason = "time_budget";
                break;
            }
            state.budget.pages_attempted += 1;
            let candidate = &mut state.candidates[index];
            match timeout_at(
                deadline.min(Instant::now() + Duration::from_secs(15)),
                tools.open_page(&candidate.url),
            )
            .await
            {
                Ok(Ok(material)) => {
                    let canonical = canonicalize_url(&material.url);
                    if input
                        .materials
                        .iter()
                        .any(|m| m.id == material.id || canonicalize_url(&m.url) == canonical)
                    {
                        candidate.status = "duplicate";
                        candidate.message = "重定向目标已有正文快照".into();
                    } else {
                        seen.insert(canonical);
                        candidate.status = "read";
                        candidate.material_id = Some(material.id);
                        input.materials.push(material);
                        input.total_sources += 1;
                        record.pages_added += 1;
                        state.budget.pages_read += 1;
                    }
                }
                _ => {
                    candidate.status = "failed";
                    candidate.message =
                        "正文读取失败：地址受限、HTTP 错误、超时、格式不支持或正文不足".into();
                }
            }
        }
        group_materials(&mut input.materials);
        let fragments = extract_focused_fragments(&input.materials, &input.query);
        let findings = audit_materials_inner(&input.materials, &fragments, false);
        record.gaps_after = review_gaps(&input.materials, &fragments, &findings);
        let web_groups: HashSet<_> = input
            .materials
            .iter()
            .filter(|m| m.retrieval.is_some() && matches_query(&m.content, &input.query))
            .map(|m| m.dependency_group)
            .collect();
        let conflict = record
            .gaps_after
            .iter()
            .any(|g| matches!(g.kind, "numeric_difference" | "correction_cue"));
        let no_progress = record.candidates_added == 0 && record.pages_added == 0;
        state.rounds.push(record);
        if Instant::now() >= deadline {
            state.stop_reason = "time_budget";
            break;
        }
        if web_groups.len() >= 2 && !conflict {
            state.stop_reason = "coverage_target";
            break;
        }
        if state.budget.pages_attempted >= PAGE_LIMIT {
            state.stop_reason = "page_budget";
            break;
        }
        if round > 0 && no_progress {
            state.stop_reason = "no_progress";
            break;
        }
    }
    state
}

fn search_url(searxng: Option<&str>, query: &str) -> AppResult<Url> {
    if let Some(base) = searxng {
        let mut url = checked_url(base)?;
        if url.query().is_some() || url.fragment().is_some() {
            return Err(AppError::BadRequest("SearXNG 地址不应含查询或片段".into()));
        }
        url.set_path(&format!("{}/search", url.path().trim_end_matches('/')));
        url.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("format", "json");
        return Ok(url);
    }
    let mut url = Url::parse("https://api.gdeltproject.org/api/v2/doc/doc").unwrap();
    // Only literal terms, never GDELT operators injected through a title.
    let literal = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .take(16)
        .collect::<Vec<_>>()
        .join(" ");
    if literal.is_empty() {
        return Err(AppError::BadRequest("没有可检索的词项".into()));
    }
    url.query_pairs_mut()
        .append_pair("query", &literal)
        .append_pair("mode", "artlist")
        .append_pair("format", "json")
        .append_pair("maxrecords", "12")
        .append_pair("timespan", "1month")
        .append_pair("sort", "hybridrel");
    Ok(url)
}

fn parse_hits(value: Value, searxng: bool) -> AppResult<Vec<SearchHit>> {
    let rows = value[if searxng { "results" } else { "articles" }]
        .as_array()
        .ok_or_else(|| AppError::Upstream("搜索响应缺少结果列表".into()))?;
    if searxng
        && rows.is_empty()
        && value["unresponsive_engines"]
            .as_array()
            .is_some_and(|r| !r.is_empty())
    {
        return Err(AppError::Upstream("搜索引擎不可用".into()));
    }
    Ok(rows
        .iter()
        .take(12)
        .filter_map(|r| {
            let url = r["url"].as_str()?;
            if url.len() > 4096 || checked_url(url).is_err() {
                return None;
            }
            Some(SearchHit {
                url: url.into(),
                title: r["title"]
                    .as_str()
                    .unwrap_or(url)
                    .chars()
                    .take(300)
                    .collect(),
            })
        })
        .collect())
}

#[async_trait]
impl ResearchTools for PublicTools {
    async fn search(&self, query: &str) -> AppResult<Vec<SearchHit>> {
        let url = search_url(self.searxng.as_deref(), query)?;
        let response = if self.searxng.is_some() {
            // An operator-configured search service may be self-hosted. Never follow its redirects.
            reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(20))
                .build()?
                .get(url)
                .header(header::ACCEPT_ENCODING, "identity")
                .send()
                .await?
        } else {
            get_public(url).await?
        };
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AppError::RateLimited("搜索服务限流".into()));
        }
        if !response.status().is_success() {
            return Err(AppError::Upstream("搜索服务 HTTP 错误".into()));
        }
        let bytes = net::read_response_limited(response, MAX_BYTES).await?;
        let value = serde_json::from_slice(&bytes)
            .map_err(|_| AppError::Upstream("搜索响应不是 JSON".into()))?;
        parse_hits(value, self.searxng.is_some())
    }

    async fn open_page(&self, url: &str) -> AppResult<Material> {
        let response = get_public(checked_url(url)?).await?;
        if !response.status().is_success() {
            return Err(AppError::Upstream("页面 HTTP 错误".into()));
        }
        let final_url = response.url().to_string();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if !matches!(
            content_type.as_str(),
            "text/html" | "text/plain" | "application/xhtml+xml"
        ) {
            return Err(AppError::Upstream("页面不是支持的文本格式".into()));
        }
        let bytes = net::read_response_limited(response, MAX_BYTES).await?;
        let raw_sha256 = hex::encode(Sha256::digest(&bytes));
        let raw = String::from_utf8(bytes)
            .map_err(|_| AppError::Upstream("页面不是 UTF-8 文本".into()))?;
        let (text, title, published) = if content_type == "text/plain" {
            (
                extractor::normalize_whitespace(&raw),
                final_url.clone(),
                None,
            )
        } else {
            let html = Html::parse_document(&raw);
            let title = html
                .select(&Selector::parse("title").unwrap())
                .next()
                .map(|n| n.text().collect::<String>())
                .unwrap_or_else(|| final_url.clone());
            let published =
                page_metadata(raw.as_bytes(), &checked_url(&final_url)?)["declared_published_at"]
                    .as_str()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|t| t.with_timezone(&Utc));
            (extractor::extract_text_from_html(&raw), title, published)
        };
        if text.chars().filter(|c| !c.is_whitespace()).count() < 80 {
            return Err(AppError::Upstream("页面正文不足".into()));
        }
        let content: String = text.chars().take(MAX_TEXT).collect();
        let canonical_url = canonicalize_url(&final_url);
        let hash = Sha256::digest(canonical_url.as_bytes());
        let id = Uuid::from_bytes(hash[..16].try_into().unwrap());
        Ok(Material {
            id,
            title: title.chars().take(300).collect(),
            url: final_url.clone(),
            canonical_url,
            publisher: publisher_family(Some(&final_url), &final_url, None, None),
            reported_published_at: published,
            retrieved_at: Utc::now(),
            text_sha256: text_hash(&content),
            full_text_sha256: text_hash(&text),
            content_truncated: content.len() < text.len(),
            content,
            dependency_group: 0,
            document: None,
            retrieval: Some(
                json!({"requested_url":url,"final_url":final_url,"content_type":content_type,"response_sha256":raw_sha256}),
            ),
        })
    }
}

async fn get_public(mut url: Url) -> AppResult<reqwest::Response> {
    for redirect in 0..=5 {
        checked_url(url.as_str())?;
        let host = url.host_str().unwrap();
        let mut addresses: Vec<_> =
            tokio::net::lookup_host((host, url.port_or_known_default().unwrap_or(80)))
                .await
                .map_err(|_| AppError::Upstream("页面地址解析失败".into()))?
                .collect();
        // Some TUN clients synthesize 198.18/15 for all public names. Resolve
        // only that specific mapping through fixed HTTPS DNS, never allow the
        // synthetic/private address itself or relax checks for other ranges.
        if !addresses.is_empty() && addresses.iter().all(|a| is_fake_ip(a.ip())) {
            addresses = resolve_public_dns(host, url.port_or_known_default().unwrap_or(80)).await?;
        }
        if addresses.is_empty()
            || addresses
                .iter()
                .any(|a| net::ip_is_private_or_reserved(a.ip()))
        {
            return Err(AppError::BadRequest("禁止读取私网或保留地址".into()));
        }
        // Pin the checked DNS answers to this request, including every redirect hop.
        let client = reqwest::Client::builder()
            .no_proxy()
            .gzip(false)
            .brotli(false)
            .deflate(false)
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(host, &addresses)
            .timeout(Duration::from_secs(15))
            .build()?;
        let response = client
            .get(url.clone())
            .header(header::USER_AGENT, "infofetcher/0.1 (public research)")
            .header(header::ACCEPT_ENCODING, "identity")
            .send()
            .await?;
        if !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            return Ok(response);
        }
        if redirect == 5 {
            break;
        }
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| AppError::Upstream("重定向缺少目标".into()))?;
        url = url
            .join(location)
            .map_err(|_| AppError::Upstream("重定向目标无效".into()))?;
    }
    Err(AppError::Upstream("重定向过多".into()))
}

fn is_fake_ip(ip: std::net::IpAddr) -> bool {
    matches!(ip, std::net::IpAddr::V4(ip) if ip.octets()[0] == 198 && matches!(ip.octets()[1], 18 | 19))
}

fn public_dns_answers(value: &Value, port: u16) -> AppResult<Vec<std::net::SocketAddr>> {
    if value["Status"] != 0 || value["TC"] == true {
        return Err(AppError::Upstream("公共 DNS 未返回完整结果".into()));
    }
    let addresses: Vec<_> = value["Answer"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| a["type"] == 1)
        .filter_map(|a| a["data"].as_str()?.parse::<std::net::Ipv4Addr>().ok())
        .map(|ip| std::net::SocketAddr::new(ip.into(), port))
        .collect();
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|a| net::ip_is_private_or_reserved(a.ip()))
    {
        return Err(AppError::BadRequest(
            "公共 DNS 未返回可访问的公网地址".into(),
        ));
    }
    Ok(addresses)
}

async fn resolve_public_dns(host: &str, port: u16) -> AppResult<Vec<std::net::SocketAddr>> {
    if host.parse::<std::net::IpAddr>().is_ok() || !host.contains('.') || host.ends_with(".local") {
        return Err(AppError::BadRequest("禁止解析内部或保留地址".into()));
    }
    let response = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(8))
        .build()?
        .get("https://1.1.1.1/dns-query")
        .query(&[("name", host), ("type", "A")])
        .header(header::ACCEPT, "application/dns-json")
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(AppError::Upstream("公共 DNS 请求失败".into()));
    }
    let bytes = net::read_response_limited(response, 65536).await?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::Upstream("公共 DNS 响应无效".into()))?;
    public_dns_answers(&value, port)
}

#[cfg(test)]
mod tests;
