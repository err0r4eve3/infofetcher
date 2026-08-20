CREATE TABLE event_revisions (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  story_id UUID NOT NULL REFERENCES story_clusters(id) ON DELETE CASCADE,
  article_id UUID REFERENCES articles(id) ON DELETE SET NULL,
  revision_type TEXT NOT NULL CHECK (
    revision_type IN ('ADD', 'UPDATE', 'CORRECT', 'CONTRADICT', 'CONFIRM')
  ),
  title TEXT NOT NULL,
  source_count INTEGER NOT NULL DEFAULT 1 CHECK (source_count >= 1),
  occurred_at TIMESTAMPTZ NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX idx_event_revisions_article
  ON event_revisions(article_id)
  WHERE article_id IS NOT NULL;
CREATE INDEX idx_event_revisions_story_time
  ON event_revisions(story_id, occurred_at DESC, created_at DESC);

CREATE TABLE watches (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  name TEXT NOT NULL UNIQUE,
  query TEXT NOT NULL,
  min_impact INTEGER NOT NULL DEFAULT 40 CHECK (min_impact BETWEEN 0 AND 100),
  notification_mode TEXT NOT NULL DEFAULT 'brief' CHECK (
    notification_mode IN ('off', 'brief', 'urgent')
  ),
  enabled BOOLEAN NOT NULL DEFAULT true,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO watches (name, query, min_impact, notification_mode, enabled, created_at, updated_at)
SELECT name, query, LEAST(100, GREATEST(0, min_quality)), 'brief', enabled, created_at, updated_at
FROM monitors
ON CONFLICT (name) DO NOTHING;

CREATE TABLE event_feedback (
  story_id UUID PRIMARY KEY REFERENCES story_clusters(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (
    kind IN ('more', 'less', 'known', 'irrelevant', 'bad_cluster')
  ),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Seed one durable baseline revision for recent events. Older clusters remain available
-- through the compatibility reader and receive revisions when new observations arrive.
INSERT INTO event_revisions (story_id, article_id, revision_type, title, source_count, occurred_at)
SELECT DISTINCT ON (sc.id)
  sc.id,
  a.id,
  'ADD',
  a.title,
  GREATEST(sc.source_count, 1),
  COALESCE(a.published_at, a.fetched_at)
FROM story_clusters sc
JOIN articles a ON a.story_id = sc.id
WHERE COALESCE(sc.last_published_at, sc.updated_at) >= now() - interval '30 days'
ORDER BY sc.id, COALESCE(a.published_at, a.fetched_at), a.id
ON CONFLICT (article_id) WHERE article_id IS NOT NULL DO NOTHING;
