ALTER TABLE articles
  ADD COLUMN needs_recluster BOOLEAN NOT NULL DEFAULT false;

CREATE INDEX idx_articles_needs_recluster
  ON articles(updated_at)
  WHERE needs_recluster;
