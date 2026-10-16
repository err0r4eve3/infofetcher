use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

use tantivy::{
    Index, IndexReader, IndexWriter, ReloadPolicy, Term,
    collector::TopDocs,
    doc,
    query::QueryParser,
    schema::{
        Field, IndexRecordOption, STORED, STRING, Schema, TantivyDocument, TextFieldIndexing,
        TextOptions, Value,
    },
    tokenizer::{Token, TokenStream, Tokenizer},
};
use uuid::Uuid;

use crate::{error::AppResult, models::SearchDocument};

const WRITER_MEMORY_BYTES: usize = 50_000_000;
const TEXT_TOKENIZER: &str = "cjk_bigram";

/// 进程内只用一个常驻 `IndexWriter`，首次写入时创建。每批新建再丢弃 writer 会留下无人管理的
/// 后台合并线程，段数失控（2026-10-14 生产索引碎成 12,095 段，合并线程占满 CPU）。
#[derive(Clone)]
pub struct SearchIndex {
    index: Index,
    reader: IndexReader,
    fields: SearchFields,
    writer: Arc<Mutex<Option<IndexWriter<TantivyDocument>>>>,
    rebuilt: bool,
}

#[derive(Clone, Copy)]
struct SearchFields {
    id: Field,
    feed_title: Field,
    title: Field,
    content: Field,
    url: Field,
    published_at: Field,
}

impl SearchIndex {
    pub fn open(path: impl AsRef<Path>) -> AppResult<Self> {
        let path = path.as_ref();
        fs::create_dir_all(path).map_err(anyhow::Error::from)?;
        let (schema, fields) = build_schema();
        let (index, rebuilt) = match Index::open_in_dir(path) {
            Ok(index) if index.schema() == schema => (index, false),
            existing => {
                // 索引结构变了（如更换分词器）或目录为空：清空后新建，由调用方决定是否重建。
                if existing.is_ok() {
                    tracing::warn!(path = %path.display(), "search index schema changed; recreating");
                }
                fs::remove_dir_all(path).map_err(anyhow::Error::from)?;
                fs::create_dir_all(path).map_err(anyhow::Error::from)?;
                (Index::create_in_dir(path, schema)?, existing.is_ok())
            }
        };
        index
            .tokenizers()
            .register(TEXT_TOKENIZER, CjkBigramTokenizer::default());
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        Ok(Self {
            index,
            reader,
            fields,
            writer: Arc::new(Mutex::new(None)),
            rebuilt,
        })
    }

    /// 打开时旧索引因结构变化被清空，需要从数据库重建。
    pub fn needs_rebuild(&self) -> bool {
        self.rebuilt
    }

    pub fn index_articles<'a>(
        &self,
        articles: impl IntoIterator<Item = &'a SearchDocument>,
    ) -> AppResult<()> {
        self.with_writer(|writer| {
            let mut has_changes = false;
            for article in articles {
                self.write_article(writer, article)?;
                has_changes = true;
            }
            if has_changes {
                self.commit(writer)?;
            }
            Ok(())
        })
    }

    pub fn clear(&self) -> AppResult<()> {
        self.with_writer(|writer| {
            writer.delete_all_documents()?;
            self.commit(writer)
        })
    }

    pub fn delete_articles(&self, ids: &[Uuid]) -> AppResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        self.with_writer(|writer| {
            for id in ids {
                writer.delete_term(Term::from_field_text(self.fields.id, &id.to_string()));
            }
            self.commit(writer)
        })
    }

    fn write_article(&self, writer: &mut IndexWriter, article: &SearchDocument) -> AppResult<()> {
        writer.delete_term(Term::from_field_text(
            self.fields.id,
            &article.id.to_string(),
        ));
        let published_at = article
            .published_at
            .map(|value| value.timestamp())
            .unwrap_or_default();
        writer.add_document(doc!(
            self.fields.id => article.id.to_string(),
            self.fields.feed_title => article.feed_title.clone(),
            self.fields.title => article.title.clone(),
            self.fields.content => article.content.clone(),
            self.fields.url => article.url.clone(),
            self.fields.published_at => published_at,
        ))?;
        Ok(())
    }

    fn with_writer<T>(
        &self,
        operation: impl FnOnce(&mut IndexWriter<TantivyDocument>) -> AppResult<T>,
    ) -> AppResult<T> {
        let mut guard = self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.is_none() {
            *guard = Some(self.index.writer(WRITER_MEMORY_BYTES)?);
        }
        let writer = guard.as_mut().expect("writer was just created");
        let result = operation(writer);
        if result.is_err() {
            // 失败的批次可能留下未提交的增删；丢弃它们，避免混进下一次提交。
            writer.rollback()?;
        }
        result
    }

    fn commit(&self, writer: &mut IndexWriter<TantivyDocument>) -> AppResult<()> {
        writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    pub fn search(&self, query: &str, limit: usize) -> AppResult<Vec<Uuid>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        // 其他进程（如 `reindex` 命令）也可能提交，查询前按需刷新。
        self.reader.reload()?;
        let searcher = self.reader.searcher();
        let mut parser = QueryParser::for_index(
            &self.index,
            vec![
                self.fields.title,
                self.fields.content,
                self.fields.feed_title,
            ],
        );
        parser.set_field_boost(self.fields.title, 3.0);
        parser.set_field_boost(self.fields.feed_title, 1.5);
        let query = parser
            .parse_query(query)
            .map_err(|err| crate::error::AppError::BadRequest(format!("搜索语法错误: {err}")))?;
        let top_docs = searcher.search(&query, &TopDocs::with_limit(limit))?;
        let mut ids = Vec::new();
        for (_score, address) in top_docs {
            let document = searcher.doc::<TantivyDocument>(address)?;
            if let Some(value) = document
                .get_first(self.fields.id)
                .and_then(|value| value.as_str())
                && let Ok(id) = Uuid::parse_str(value)
            {
                ids.push(id);
            }
        }
        Ok(ids)
    }
}

fn build_schema() -> (Schema, SearchFields) {
    let mut builder = Schema::builder();
    let id = builder.add_text_field("id", STRING | STORED);
    let text = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(TEXT_TOKENIZER)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    let feed_title = builder.add_text_field("feed_title", text.clone() | STORED);
    let title = builder.add_text_field("title", text.clone() | STORED);
    let content = builder.add_text_field("content", text);
    let url = builder.add_text_field("url", STRING | STORED);
    let published_at = builder.add_i64_field("published_at", STORED);
    let schema = builder.build();
    (
        schema,
        SearchFields {
            id,
            feed_title,
            title,
            content,
            url,
            published_at,
        },
    )
}

/// 中日韩文字按相邻两字切分（单字成段时保留单字），其他字母数字按整词切分并转小写。
/// 默认分词器会把整段中文当成一个词，导致中文检索几乎全部落空。
#[derive(Clone, Default)]
struct CjkBigramTokenizer {
    tokens: Vec<Token>,
}

struct VecTokenStream<'a> {
    tokens: &'a [Token],
    cursor: Option<usize>,
}

impl TokenStream for VecTokenStream<'_> {
    fn advance(&mut self) -> bool {
        let next = self.cursor.map_or(0, |index| index + 1);
        self.cursor = Some(next);
        next < self.tokens.len()
    }

    fn token(&self) -> &Token {
        &self.tokens[self.cursor.unwrap_or(0)]
    }

    fn token_mut(&mut self) -> &mut Token {
        unreachable!("tokens are not modified by filters")
    }
}

impl Tokenizer for CjkBigramTokenizer {
    type TokenStream<'a> = VecTokenStream<'a>;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        self.tokens.clear();
        let push = |tokens: &mut Vec<Token>, from: usize, to: usize, text: String| {
            let position = tokens.len();
            tokens.push(Token {
                offset_from: from,
                offset_to: to,
                position,
                text,
                position_length: 1,
            });
        };
        let chars = text.char_indices().collect::<Vec<_>>();
        let end_of = |index: usize| chars.get(index + 1).map_or(text.len(), |(offset, _)| *offset);
        let mut index = 0;
        while index < chars.len() {
            let (start, character) = chars[index];
            if is_cjk(character) {
                let mut run_end = index;
                while run_end + 1 < chars.len() && is_cjk(chars[run_end + 1].1) {
                    run_end += 1;
                }
                if run_end == index {
                    push(&mut self.tokens, start, end_of(index), character.to_string());
                } else {
                    for left in index..run_end {
                        let to = end_of(left + 1);
                        push(&mut self.tokens, chars[left].0, to, text[chars[left].0..to].to_string());
                    }
                }
                index = run_end + 1;
            } else if character.is_alphanumeric() {
                let mut run_end = index;
                while run_end + 1 < chars.len()
                    && chars[run_end + 1].1.is_alphanumeric()
                    && !is_cjk(chars[run_end + 1].1)
                {
                    run_end += 1;
                }
                let to = end_of(run_end);
                push(&mut self.tokens, start, to, text[start..to].to_lowercase());
                index = run_end + 1;
            } else {
                index += 1;
            }
        }
        VecTokenStream {
            tokens: &self.tokens,
            cursor: None,
        }
    }
}

fn is_cjk(character: char) -> bool {
    matches!(
        character as u32,
        0x3040..=0x30FF   // 平假名、片假名
            | 0x3400..=0x4DBF // 扩展 A
            | 0x4E00..=0x9FFF // 基本汉字
            | 0xAC00..=0xD7AF // 韩文音节
            | 0xF900..=0xFAFF // 兼容汉字
            | 0x20000..=0x2FA1F // 扩展 B 及以后
    )
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc, Barrier,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::Duration,
    };

    use chrono::Utc;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn indexes_and_searches_article_content() {
        let dir = tempdir().unwrap();
        let index = SearchIndex::open(dir.path()).unwrap();
        let id = Uuid::new_v4();
        let article = SearchDocument {
            id,
            feed_title: "技术博客".to_string(),
            title: "Rust RSS 阅读器".to_string(),
            content: "Tantivy 提供高性能全文搜索".to_string(),
            url: "https://example.com".to_string(),
            published_at: Some(Utc::now()),
        };
        index.index_articles([&article]).unwrap();
        assert_eq!(index.search("Tantivy", 10).unwrap(), vec![id]);
    }

    #[test]
    fn indexes_multiple_articles_in_one_batch() {
        let dir = tempdir().unwrap();
        let index = SearchIndex::open(dir.path()).unwrap();
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        let articles = vec![
            SearchDocument {
                id: first_id,
                feed_title: "技术博客".to_string(),
                title: "Rust RSS 阅读器".to_string(),
                content: "Tantivy 提供高性能全文搜索".to_string(),
                url: "https://example.com/1".to_string(),
                published_at: Some(Utc::now()),
            },
            SearchDocument {
                id: second_id,
                feed_title: "技术博客".to_string(),
                title: "Batch indexing".to_string(),
                content: "Batch commit reduces disk writes".to_string(),
                url: "https://example.com/2".to_string(),
                published_at: Some(Utc::now()),
            },
        ];

        index.index_articles(&articles).unwrap();

        assert_eq!(index.search("Tantivy", 10).unwrap(), vec![first_id]);
        assert_eq!(index.search("Batch", 10).unwrap(), vec![second_id]);
    }

    fn tokens(text: &str) -> Vec<String> {
        let mut tokenizer = CjkBigramTokenizer::default();
        let mut stream = tokenizer.token_stream(text);
        let mut out = Vec::new();
        while let Some(token) = stream.next() {
            out.push(token.text.clone());
        }
        out
    }

    #[test]
    fn splits_cjk_into_bigrams_and_latin_into_words() {
        assert_eq!(tokens("美国制裁伊朗"), ["美国", "国制", "制裁", "裁伊", "伊朗"]);
        assert_eq!(tokens("iPhone18发布，Apple"), ["iphone18", "发布", "apple"]);
        assert_eq!(tokens("美 Iran"), ["美", "iran"]);
        assert_eq!(tokens("东京タワー"), ["东京", "京タ", "タワ", "ワー"]);
    }

    #[test]
    fn finds_chinese_substrings_inside_long_titles() {
        let dir = tempdir().unwrap();
        let index = SearchIndex::open(dir.path()).unwrap();
        let id = Uuid::new_v4();
        let other = Uuid::new_v4();
        let documents = [
            SearchDocument {
                id,
                feed_title: "快讯".to_string(),
                title: "美国总统称伊朗石油部长刚刚辞职".to_string(),
                content: "伊朗没有经济，没有石油".to_string(),
                url: "https://example.com/iran".to_string(),
                published_at: Some(Utc::now()),
            },
            SearchDocument {
                id: other,
                feed_title: "快讯".to_string(),
                title: "伊拉克暂停航班".to_string(),
                content: "朗读比赛".to_string(),
                url: "https://example.com/iraq".to_string(),
                published_at: Some(Utc::now()),
            },
        ];
        index.index_articles(&documents).unwrap();
        assert_eq!(index.search("伊朗", 10).unwrap(), vec![id]);
        assert_eq!(index.search("石油部长", 10).unwrap(), vec![id]);
        assert!(index.search("伊朗部长", 10).unwrap().is_empty());
    }

    #[test]
    fn recreates_index_when_schema_changes() {
        let dir = tempdir().unwrap();
        let mut old = Schema::builder();
        old.add_text_field("title", tantivy::schema::TEXT);
        Index::create_in_dir(dir.path(), old.build()).unwrap();
        let index = SearchIndex::open(dir.path()).unwrap();
        assert!(index.needs_rebuild());
        drop(index);
        assert!(!SearchIndex::open(dir.path()).unwrap().needs_rebuild());
    }

    #[test]
    fn reuses_one_writer_across_batches() {
        let dir = tempdir().unwrap();
        let index = SearchIndex::open(dir.path()).unwrap();
        let mut ids = Vec::new();
        for batch in 0..40 {
            let id = Uuid::new_v4();
            ids.push(id);
            let article = SearchDocument {
                id,
                feed_title: "batches".to_string(),
                title: format!("batch{batch} headline"),
                content: "steady segment count".to_string(),
                url: format!("https://example.com/{batch}"),
                published_at: Some(Utc::now()),
            };
            index.index_articles([&article]).unwrap();
        }
        // A second writer on the same directory fails while the first is alive,
        // so every batch above went through the same writer.
        assert!(index.index.writer::<TantivyDocument>(WRITER_MEMORY_BYTES).is_err());
        assert_eq!(index.search("batch39", 10).unwrap(), vec![ids[39]]);
        assert_eq!(index.search("steady", 50).unwrap().len(), 40);
    }

    #[test]
    fn serializes_concurrent_write_operations() {
        let directory = tempdir().unwrap();
        let index = SearchIndex::open(directory.path()).unwrap();
        let barrier = Arc::new(Barrier::new(4));
        let active_writers = Arc::new(AtomicUsize::new(0));
        let max_writers = Arc::new(AtomicUsize::new(0));

        thread::scope(|scope| {
            for _ in 0..4 {
                let index = index.clone();
                let barrier = barrier.clone();
                let active_writers = active_writers.clone();
                let max_writers = max_writers.clone();
                scope.spawn(move || {
                    barrier.wait();
                    index
                        .with_writer(|_writer| {
                            let current = active_writers.fetch_add(1, Ordering::SeqCst) + 1;
                            max_writers.fetch_max(current, Ordering::SeqCst);
                            thread::sleep(Duration::from_millis(10));
                            active_writers.fetch_sub(1, Ordering::SeqCst);
                            Ok(())
                        })
                        .unwrap();
                });
            }
        });

        assert_eq!(max_writers.load(Ordering::SeqCst), 1);
    }
}
