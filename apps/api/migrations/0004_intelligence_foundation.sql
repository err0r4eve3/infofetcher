CREATE EXTENSION IF NOT EXISTS pg_trgm;

ALTER TABLE feeds
  ADD COLUMN source_type TEXT NOT NULL DEFAULT 'rss',
  ADD COLUMN language TEXT,
  ADD COLUMN region TEXT,
  ADD COLUMN quality_score INTEGER NOT NULL DEFAULT 50 CHECK (quality_score BETWEEN 0 AND 100),
  ADD COLUMN last_success_at TIMESTAMPTZ,
  ADD COLUMN next_fetch_at TIMESTAMPTZ,
  ADD COLUMN consecutive_failures INTEGER NOT NULL DEFAULT 0,
  ADD COLUMN websub_hub_url TEXT,
  ADD COLUMN websub_topic_url TEXT,
  ADD COLUMN websub_secret TEXT,
  ADD COLUMN websub_lease_expires_at TIMESTAMPTZ;

UPDATE feeds
SET last_success_at = last_fetched_at,
    next_fetch_at = COALESCE(
      last_fetched_at + (fetch_interval_minutes || ' minutes')::interval,
      now()
    )
WHERE next_fetch_at IS NULL;

CREATE TABLE story_clusters (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  representative_title TEXT NOT NULL,
  story_key TEXT NOT NULL,
  source_count INTEGER NOT NULL DEFAULT 1,
  quality_score INTEGER NOT NULL DEFAULT 50 CHECK (quality_score BETWEEN 0 AND 100),
  first_published_at TIMESTAMPTZ,
  last_published_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_story_clusters_key_updated
  ON story_clusters(story_key, updated_at DESC);
CREATE INDEX idx_story_clusters_last_published
  ON story_clusters(last_published_at DESC NULLS LAST);

ALTER TABLE articles DROP CONSTRAINT IF EXISTS articles_url_hash_key;
ALTER TABLE articles
  ADD COLUMN canonical_url TEXT,
  ADD COLUMN content_hash TEXT,
  ADD COLUMN language TEXT,
  ADD COLUMN quality_score INTEGER NOT NULL DEFAULT 50 CHECK (quality_score BETWEEN 0 AND 100),
  ADD COLUMN story_id UUID REFERENCES story_clusters(id) ON DELETE SET NULL;

UPDATE articles
SET canonical_url = url,
    content_hash = url_hash
WHERE canonical_url IS NULL OR content_hash IS NULL;

ALTER TABLE articles
  ALTER COLUMN canonical_url SET NOT NULL,
  ALTER COLUMN content_hash SET NOT NULL;

CREATE UNIQUE INDEX idx_articles_feed_url_hash_unique
  ON articles(feed_id, url_hash);
CREATE INDEX idx_articles_url_hash ON articles(url_hash);
CREATE INDEX idx_articles_story ON articles(story_id);
CREATE INDEX idx_articles_quality_published
  ON articles(quality_score DESC, published_at DESC NULLS LAST);
CREATE INDEX idx_articles_title_trgm
  ON articles USING gin (title gin_trgm_ops);

ALTER TABLE summaries
  ADD COLUMN confidence TEXT,
  ADD COLUMN confidence_reason TEXT,
  ADD COLUMN citations JSONB NOT NULL DEFAULT '[]'::jsonb;

CREATE TABLE worker_jobs (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  kind TEXT NOT NULL CHECK (kind IN ('fetch_feed', 'summarize_article')),
  feed_id UUID REFERENCES feeds(id) ON DELETE CASCADE,
  article_id UUID REFERENCES articles(id) ON DELETE CASCADE,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'running', 'done', 'failed')),
  attempts INTEGER NOT NULL DEFAULT 0,
  max_attempts INTEGER NOT NULL DEFAULT 5,
  available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  locked_at TIMESTAMPTZ,
  last_error TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  completed_at TIMESTAMPTZ,
  CHECK (
    (kind = 'fetch_feed' AND feed_id IS NOT NULL AND article_id IS NULL)
    OR
    (kind = 'summarize_article' AND article_id IS NOT NULL AND feed_id IS NULL)
  )
);

CREATE INDEX idx_worker_jobs_due
  ON worker_jobs(status, available_at, created_at);
CREATE UNIQUE INDEX idx_worker_jobs_active_feed
  ON worker_jobs(kind, feed_id)
  WHERE status IN ('pending', 'running') AND feed_id IS NOT NULL;
CREATE UNIQUE INDEX idx_worker_jobs_active_article
  ON worker_jobs(kind, article_id)
  WHERE status IN ('pending', 'running') AND article_id IS NOT NULL;

ALTER TABLE summary_jobs
  ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0,
  ADD COLUMN available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  ADD COLUMN started_at TIMESTAMPTZ,
  ADD COLUMN completed_at TIMESTAMPTZ;

CREATE TABLE monitors (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  name TEXT NOT NULL UNIQUE,
  query TEXT NOT NULL,
  min_quality INTEGER NOT NULL DEFAULT 0 CHECK (min_quality BETWEEN 0 AND 100),
  unread_only BOOLEAN NOT NULL DEFAULT false,
  enabled BOOLEAN NOT NULL DEFAULT true,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE source_feedback (
  feed_id UUID PRIMARY KEY REFERENCES feeds(id) ON DELETE CASCADE,
  weight INTEGER NOT NULL DEFAULT 0 CHECK (weight BETWEEN -50 AND 50),
  muted BOOLEAN NOT NULL DEFAULT false,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
