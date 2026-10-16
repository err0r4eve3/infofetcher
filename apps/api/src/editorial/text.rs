//! 评审输入的正文采样，以及“此前进展”使用的小型 BM25 检索。

use std::collections::{HashMap, HashSet};

/// 截取不超过 `max_chars` 的正文。长文保留开头、中段和结尾，避免丢掉结论。
pub fn sample_content(content: &str, max_chars: usize, head_middle_tail: bool) -> String {
    let text = content.trim();
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    if !head_middle_tail {
        return chars[..max_chars]
            .iter()
            .collect::<String>()
            .trim_end()
            .to_string();
    }
    const MARKERS: [&str; 3] = ["[开头]\n", "\n\n[中段]\n", "\n\n[结尾]\n"];
    let available = max_chars.saturating_sub(MARKERS.iter().map(|m| m.chars().count()).sum());
    let opening = available * 4 / 10;
    let middle = available * 3 / 10;
    let closing = available - opening - middle;
    let middle_start = (chars.len() / 2).saturating_sub(middle / 2);
    let slice = |start: usize, len: usize| chars[start..start + len].iter().collect::<String>();
    format!(
        "{}{}{}{}{}{}",
        MARKERS[0],
        slice(0, opening).trim_end(),
        MARKERS[1],
        slice(middle_start, middle).trim(),
        MARKERS[2],
        slice(chars.len() - closing, closing).trim_start()
    )
}

/// 拉丁文按词（转小写、去停用词），中日韩文按相邻两字切分；不需要额外分词器。
pub fn tokenize(text: &str) -> Vec<String> {
    const STOP_WORDS: &[&str] = &[
        "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "in", "is", "it", "of",
        "on", "or", "the", "to", "was", "with", "says", "said", "after", "amid", "over", "new",
    ];
    let mut terms = Vec::new();
    let mut word = String::new();
    let mut cjk = Vec::<char>::new();
    let flush_word = |word: &mut String, terms: &mut Vec<String>| {
        if !word.is_empty() && !STOP_WORDS.contains(&word.as_str()) {
            terms.push(std::mem::take(word));
        }
        word.clear();
    };
    let flush_cjk = |run: &mut Vec<char>, terms: &mut Vec<String>| {
        terms.extend(run.windows(2).map(|pair| pair.iter().collect::<String>()));
        run.clear();
    };
    for character in text.to_lowercase().chars() {
        if is_cjk(character) {
            flush_word(&mut word, &mut terms);
            cjk.push(character);
        } else if character.is_alphanumeric() {
            flush_cjk(&mut cjk, &mut terms);
            word.push(character);
        } else {
            flush_word(&mut word, &mut terms);
            flush_cjk(&mut cjk, &mut terms);
        }
    }
    flush_word(&mut word, &mut terms);
    flush_cjk(&mut cjk, &mut terms);
    terms
}

fn is_cjk(character: char) -> bool {
    matches!(character as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF)
}

/// 一篇可检索的文档；`terms` 已含标题加权。
pub struct Document<T> {
    pub value: T,
    terms: HashMap<String, u32>,
    length: u32,
}

impl<T> Document<T> {
    /// 标题计两次，突出标题词。
    pub fn new(value: T, title: &str, body: &str) -> Self {
        let mut terms = HashMap::<String, u32>::new();
        for term in tokenize(title)
            .into_iter()
            .chain(tokenize(title))
            .chain(tokenize(body))
        {
            *terms.entry(term).or_default() += 1;
        }
        let length = terms.values().sum();
        Self {
            value,
            terms,
            length,
        }
    }
}

/// BM25（k1=1.5，b=0.75）。只返回至少命中 `min_matched_terms` 个不同查询词的文档，
/// 避免单个常见 bigram 带来的偶然命中。
pub fn rank<'a, T>(
    documents: &'a [Document<T>],
    query: &str,
    min_matched_terms: usize,
    mut eligible: impl FnMut(&T) -> bool,
) -> Vec<(&'a Document<T>, f32)> {
    let query_terms = tokenize(query).into_iter().collect::<HashSet<_>>();
    if documents.is_empty() || query_terms.is_empty() {
        return Vec::new();
    }
    let count = documents.len() as f32;
    let average_length =
        (documents.iter().map(|doc| doc.length as f32).sum::<f32>() / count).max(1.0);
    let idf = query_terms
        .iter()
        .map(|term| {
            let frequency = documents
                .iter()
                .filter(|doc| doc.terms.contains_key(term))
                .count() as f32;
            (
                term,
                (1.0 + (count - frequency + 0.5) / (frequency + 0.5)).ln(),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut scored = documents
        .iter()
        .filter(|doc| eligible(&doc.value))
        .filter_map(|doc| {
            let normalization = 1.5 * (0.25 + 0.75 * doc.length as f32 / average_length);
            let mut matched = 0;
            let mut score = 0.0;
            for term in &query_terms {
                let Some(&frequency) = doc.terms.get(term) else {
                    continue;
                };
                matched += 1;
                let frequency = frequency as f32;
                score += idf[term] * frequency * 2.5 / (frequency + normalization);
            }
            (matched >= min_matched_terms && score > 0.0).then_some((doc, score))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| right.1.total_cmp(&left.1));
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_content_is_unchanged() {
        assert_eq!(sample_content("  短文  ", 100, true), "短文");
    }

    #[test]
    fn prefix_sampling_keeps_the_opening() {
        let text = "a".repeat(50) + &"b".repeat(50);
        assert_eq!(sample_content(&text, 50, false), "a".repeat(50));
    }

    #[test]
    fn head_middle_tail_sampling_keeps_the_conclusion() {
        let text = format!(
            "{}{}{}",
            "开".repeat(400),
            "中".repeat(400),
            "结".repeat(400)
        );
        let sampled = sample_content(&text, 300, true);
        assert!(sampled.chars().count() <= 300);
        assert!(sampled.starts_with("[开头]\n开"));
        assert!(sampled.contains("[中段]\n中"));
        assert!(sampled.ends_with('结'));
    }

    #[test]
    fn tokenizes_latin_words_and_cjk_bigrams() {
        assert_eq!(
            tokenize("The vLLM 0.9 发布了"),
            vec!["vllm", "0", "9", "发布", "布了"]
        );
    }

    #[test]
    fn ranking_requires_multiple_matched_terms_and_respects_eligibility() {
        let documents = vec![
            Document::new(1, "伊朗石油部长访问俄罗斯", "双方讨论能源合作"),
            Document::new(2, "伊朗总统选举", "投票率创新低"),
            Document::new(3, "石油价格上涨", ""),
            Document::new(4, "伊朗石油出口制裁", "美国宣布新制裁"),
        ];
        let ranked = rank(&documents, "伊朗石油部长", 2, |value| *value != 4);
        let ids = ranked.iter().map(|(doc, _)| doc.value).collect::<Vec<_>>();
        assert_eq!(ids.first(), Some(&1));
        assert!(!ids.contains(&4), "ineligible documents are excluded");
        assert!(!ids.contains(&2), "a single shared bigram is not enough");
    }
}
