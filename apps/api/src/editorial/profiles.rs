//! 编辑 Profile：一套可复用的处理规则——什么内容适用、按什么标准评分、额外写哪些区块。
//! 路由说明和评分标准放在 `apps/api/profiles/<id>/*.md`，编译期载入，改提示词不用改代码。

pub struct Profile {
    pub id: &'static str,
    pub name: &'static str,
    pub match_prompt: &'static str,
    pub analysis_prompt: &'static str,
    /// 交给评审的正文上限与采样方式。
    pub content_chars: usize,
    pub head_middle_tail: bool,
    /// 是否输出“方法与启示”区块。
    pub takeaway: bool,
}

pub const DEFAULT_PROFILE: &str = "world-news";

pub const PROFILES: &[Profile] = &[
    Profile {
        id: "world-news",
        name: "时事要闻",
        match_prompt: include_str!("../../profiles/world-news/match.md"),
        analysis_prompt: include_str!("../../profiles/world-news/analysis.md"),
        content_chars: 1_200,
        head_middle_tail: false,
        takeaway: false,
    },
    Profile {
        id: "tech-research",
        name: "科技与研究",
        match_prompt: include_str!("../../profiles/tech-research/match.md"),
        analysis_prompt: include_str!("../../profiles/tech-research/analysis.md"),
        content_chars: 1_200,
        head_middle_tail: false,
        takeaway: false,
    },
    Profile {
        id: "long-read",
        name: "深度长文",
        match_prompt: include_str!("../../profiles/long-read/match.md"),
        analysis_prompt: include_str!("../../profiles/long-read/analysis.md"),
        content_chars: 3_000,
        head_middle_tail: true,
        takeaway: true,
    },
];

pub fn get(id: &str) -> Option<&'static Profile> {
    PROFILES.iter().find(|profile| profile.id == id)
}

/// 评审 system prompt 中的 Profile 目录。
pub fn catalog() -> String {
    PROFILES
        .iter()
        .map(|profile| {
            format!(
                "## {}：{}\n\n### 适用范围\n\n{}\n\n### 评分\n\n{}",
                profile.id,
                profile.name,
                profile.match_prompt.trim(),
                demote_headings(profile.analysis_prompt.trim())
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Profile 文件内的一级标题降为四级，保持目录层级。
fn demote_headings(markdown: &str) -> String {
    markdown
        .lines()
        .map(|line| match line.strip_prefix("# ") {
            Some(rest) => format!("#### {rest}"),
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_are_unique_and_complete() {
        let mut ids = PROFILES
            .iter()
            .map(|profile| profile.id)
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), PROFILES.len());
        assert!(get(DEFAULT_PROFILE).is_some());
        for profile in PROFILES {
            assert!(
                profile.match_prompt.contains("不属于本类"),
                "{} 缺少排除说明",
                profile.id
            );
            assert!(
                profile.analysis_prompt.contains("9-10"),
                "{} 缺少分档",
                profile.id
            );
            assert!(
                profile.analysis_prompt.contains("0-2"),
                "{} 缺少分档",
                profile.id
            );
        }
    }

    #[test]
    fn catalog_nests_profile_headings() {
        let catalog = catalog();
        assert!(catalog.contains("## long-read：深度长文"));
        assert!(catalog.contains("#### 评分标准"));
        assert!(!catalog.lines().any(|line| line.starts_with("# ")));
    }
}
