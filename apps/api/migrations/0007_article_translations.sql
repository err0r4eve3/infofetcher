CREATE TABLE article_translations (
  article_id UUID NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
  target_language TEXT NOT NULL CHECK (target_language IN ('zh-Hans', 'en')),
  status TEXT NOT NULL CHECK (status IN ('running', 'done', 'failed')),
  model TEXT NOT NULL,
  source_hash TEXT NOT NULL,
  prompt_version TEXT NOT NULL,
  translated_title TEXT,
  translated_content TEXT,
  error TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (article_id, target_language)
);

CREATE INDEX idx_article_translations_status_updated
  ON article_translations(status, updated_at DESC);
