-- 标题向量：事件层跨语言召回用。向量在进程内比较，不依赖 pgvector。
CREATE TABLE article_embeddings (
  article_id UUID PRIMARY KEY REFERENCES articles(id) ON DELETE CASCADE,
  model TEXT NOT NULL,
  text_hash TEXT NOT NULL,
  vector REAL[] NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 每次向量召回后的模型确认结果，用来按周评估召回阈值和确认质量。
CREATE TABLE story_link_decisions (
  id BIGSERIAL PRIMARY KEY,
  article_id UUID NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
  candidate_article_id UUID,
  candidate_story_id UUID,
  cosine REAL NOT NULL,
  cross_language BOOLEAN NOT NULL,
  verdict TEXT NOT NULL CHECK (verdict IN ('linked', 'rejected', 'error')),
  model TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_story_link_decisions_created ON story_link_decisions(created_at DESC);
CREATE INDEX idx_story_link_decisions_article ON story_link_decisions(article_id);
