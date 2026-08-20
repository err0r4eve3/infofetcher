use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

use tantivy::{
    Index, IndexWriter, Term,
    collector::TopDocs,
    doc,
    query::QueryParser,
    schema::{Field, STORED, STRING, Schema, TEXT, TantivyDocument, Value},
};
use uuid::Uuid;

use crate::{error::AppResult, models::SearchDocument};

#[derive(Clone)]
pub struct SearchIndex {
    index: Index,
    fields: SearchFields,
    write_lock: Arc<Mutex<()>>,
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
        let index = match Index::open_in_dir(path) {
            Ok(index) => index,
            Err(_) => Index::create_in_dir(path, schema)?,
        };
        Ok(Self {
            index,
            fields,
            write_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn index_articles<'a>(
        &self,
        articles: impl IntoIterator<Item = &'a SearchDocument>,
    ) -> AppResult<()> {
        self.with_writer(|| {
            let mut writer: IndexWriter<TantivyDocument> = self.index.writer(50_000_000)?;
            let mut has_changes = false;
            for article in articles {
                self.write_article(&mut writer, article)?;
                has_changes = true;
            }
            if has_changes {
                writer.commit()?;
            }
            Ok(())
        })
    }

    pub fn clear(&self) -> AppResult<()> {
        self.with_writer(|| {
            let mut writer: IndexWriter<TantivyDocument> = self.index.writer(50_000_000)?;
            writer.delete_all_documents()?;
            writer.commit()?;
            Ok(())
        })
    }

    pub fn delete_articles(&self, ids: &[Uuid]) -> AppResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        self.with_writer(|| {
            let mut writer: IndexWriter<TantivyDocument> = self.index.writer(50_000_000)?;
            for id in ids {
                writer.delete_term(Term::from_field_text(self.fields.id, &id.to_string()));
            }
            writer.commit()?;
            Ok(())
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

    fn with_writer<T>(&self, operation: impl FnOnce() -> AppResult<T>) -> AppResult<T> {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        operation()
    }

    pub fn search(&self, query: &str, limit: usize) -> AppResult<Vec<Uuid>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let reader = self.index.reader()?;
        reader.reload()?;
        let searcher = reader.searcher();
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
    let feed_title = builder.add_text_field("feed_title", TEXT | STORED);
    let title = builder.add_text_field("title", TEXT | STORED);
    let content = builder.add_text_field("content", TEXT);
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
                        .with_writer(|| {
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
