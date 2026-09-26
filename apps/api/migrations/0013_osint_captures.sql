CREATE TABLE osint_captures (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  article_id UUID NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
  story_id UUID NOT NULL REFERENCES story_clusters(id) ON DELETE CASCADE,
  captured_at TIMESTAMPTZ NOT NULL,
  sha256 TEXT NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
  body BYTEA NOT NULL CHECK (octet_length(body) <= 1048576),
  details JSONB NOT NULL
);

CREATE INDEX idx_osint_captures_article_time ON osint_captures(article_id, captured_at DESC);
