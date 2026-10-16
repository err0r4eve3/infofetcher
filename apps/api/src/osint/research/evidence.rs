use super::*;

#[derive(Serialize)]
pub(super) struct Evidence {
    pub id: String,
    pub article_id: Uuid,
    source_url: String,
    source_title: String,
    source_kind: &'static str,
    published_at: Option<DateTime<Utc>>,
    fetched_at: DateTime<Utc>,
    text_sha256: String,
    // Offsets in the exported snapshot's UTF-8 bytes, end exclusive.
    start_byte: usize,
    end_byte: usize,
    exact_excerpt: String,
}

#[derive(Serialize)]
pub(super) struct CitationAudit {
    status: &'static str,
    checked: usize,
    semantic_support: &'static str,
}

fn evidence_id(material: &Material, start: usize, end: usize) -> String {
    format!(
        "E-{}",
        text_hash(&format!(
            "{}:{}:{start}:{end}",
            material.id, material.text_sha256
        ))
    )
}

fn source_kind(material: &Material) -> &'static str {
    if material.document.is_some() {
        "document_text"
    } else if material.retrieval.is_some() {
        "page_content"
    } else {
        "article_content"
    }
}

pub(super) fn bind(materials: &[Material], fragments: &mut [Fragment]) -> AppResult<Vec<Evidence>> {
    let mut evidence = vec![];
    for fragment in fragments {
        fragment.evidence_ids.clear();
        for id in &fragment.article_ids {
            let material = materials
                .iter()
                .find(|m| m.id == *id)
                .ok_or_else(|| AppError::Anyhow(anyhow::anyhow!("引用材料不存在")))?;
            let start = material
                .content
                .find(&fragment.excerpt)
                .ok_or_else(|| AppError::Anyhow(anyhow::anyhow!("引用原文不存在")))?;
            let end = start + fragment.excerpt.len();
            let id = evidence_id(material, start, end);
            fragment.evidence_ids.push(id.clone());
            evidence.push(Evidence {
                id,
                article_id: material.id,
                source_url: material.url.clone(),
                source_title: material.title.clone(),
                source_kind: source_kind(material),
                published_at: material.reported_published_at,
                fetched_at: material.retrieved_at,
                text_sha256: material.text_sha256.clone(),
                start_byte: start,
                end_byte: end,
                exact_excerpt: fragment.excerpt.clone(),
            });
        }
    }
    Ok(evidence)
}

pub(super) fn validate(
    materials: &[Material],
    fragments: &[Fragment],
    evidence: &[Evidence],
) -> AppResult<CitationAudit> {
    let fail = || AppError::Anyhow(anyhow::anyhow!("引用完整性校验失败，未保存调查结果"));
    let mut source_ids = HashSet::new();
    for material in materials {
        if !source_ids.insert(material.id) || text_hash(&material.content) != material.text_sha256 {
            return Err(fail());
        }
    }
    let mut ids = HashSet::new();
    for e in evidence {
        let m = materials
            .iter()
            .find(|m| m.id == e.article_id)
            .ok_or_else(fail)?;
        if !ids.insert(&e.id)
            || e.id != evidence_id(m, e.start_byte, e.end_byte)
            || e.exact_excerpt.is_empty()
            || m.content.get(e.start_byte..e.end_byte) != Some(e.exact_excerpt.as_str())
            || e.text_sha256 != m.text_sha256
            || e.source_url != m.url
            || e.source_title != m.title
            || e.source_kind != source_kind(m)
            || e.published_at != m.reported_published_at
            || e.fetched_at != m.retrieved_at
        {
            return Err(fail());
        }
    }
    let mut referenced = HashSet::new();
    let mut fragment_ids = HashSet::new();
    for f in fragments {
        if f.id != format!("F-{}", text_hash(&f.excerpt))
            || !fragment_ids.insert(&f.id)
            || f.evidence_ids.len() != f.article_ids.len()
            || f.evidence_ids.is_empty()
        {
            return Err(fail());
        }
        let mut sources = HashSet::new();
        for id in &f.evidence_ids {
            let e = evidence.iter().find(|e| e.id == *id).ok_or_else(fail)?;
            if e.exact_excerpt != f.excerpt
                || !f.article_ids.contains(&e.article_id)
                || !sources.insert(e.article_id)
            {
                return Err(fail());
            }
            referenced.insert(id);
        }
    }
    if referenced.len() != evidence.len() {
        return Err(fail());
    }
    Ok(CitationAudit {
        status: "passed",
        checked: evidence.len(),
        semantic_support: "not_checked",
    })
}

#[cfg(test)]
mod tests {
    use super::super::tests::material;
    use super::*;

    #[test]
    fn unicode_citations_are_stable_and_bound_to_each_snapshot() {
        let mut materials = vec![material(
            "a",
            "前言。更正：此前公布的检测样本数量为120份，实际应为125份。",
        )];
        group_materials(&mut materials);
        let mut fragments = extract_fragments(&materials);
        let evidence = bind(&materials, &mut fragments).unwrap();
        assert_eq!(evidence[0].start_byte, "前言。".len());
        validate(&materials, &fragments, &evidence).unwrap();
        let again = bind(&materials, &mut fragments).unwrap();
        assert_eq!(evidence[0].id, again[0].id);
        materials[0].content.push('字');
        assert!(validate(&materials, &fragments, &evidence).is_err());
    }

    #[test]
    fn rejects_forged_offsets_urls_ids_and_unbound_evidence() {
        let materials = vec![material(
            "a",
            "The agency reported 12.5 percent growth in the survey.",
        )];
        let mut fragments = extract_fragments(&materials);
        let mut evidence = bind(&materials, &mut fragments).unwrap();
        evidence[0].source_url = "https://unrelated.example".into();
        assert!(validate(&materials, &fragments, &evidence).is_err());
        let mut evidence = bind(&materials, &mut fragments).unwrap();
        evidence[0].start_byte += 1;
        assert!(validate(&materials, &fragments, &evidence).is_err());
        let evidence = bind(&materials, &mut fragments).unwrap();
        fragments[0].evidence_ids[0] = "E-invented".into();
        assert!(validate(&materials, &fragments, &evidence).is_err());
        assert!(validate(&materials, &[], &evidence).is_err());
    }
}
