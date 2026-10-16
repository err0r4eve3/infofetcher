-- 编辑层：LLM 评审、信源画像、日报。见 docs/EDITORIAL_LAYER.md。
ALTER TABLE feeds
  ADD COLUMN editorial_profile TEXT
    CHECK (editorial_profile IS NULL OR editorial_profile IN ('world-news', 'tech-research', 'long-read'));

CREATE TABLE event_assessments (
  story_id UUID PRIMARY KEY REFERENCES story_clusters(id) ON DELETE CASCADE,
  materials_hash TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('done', 'failed')),
  profile TEXT NOT NULL,
  score REAL CHECK (score IS NULL OR (score >= 0 AND score <= 10)),
  reason TEXT NOT NULL DEFAULT '',
  summary TEXT NOT NULL DEFAULT '',
  tags JSONB NOT NULL DEFAULT '[]'::jsonb,
  history_note TEXT,
  history_refs JSONB NOT NULL DEFAULT '[]'::jsonb,
  community_note TEXT,
  takeaway TEXT,
  title TEXT NOT NULL DEFAULT '',
  model TEXT NOT NULL DEFAULT '',
  prompt_version TEXT NOT NULL,
  error TEXT,
  attempts INTEGER NOT NULL DEFAULT 0,
  first_seen_at TIMESTAMPTZ NOT NULL,
  assessed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_event_assessments_history
  ON event_assessments(first_seen_at DESC)
  WHERE status = 'done';
CREATE INDEX idx_event_assessments_assessed_at
  ON event_assessments(assessed_at DESC);

CREATE TABLE feed_quality_stats (
  feed_id UUID PRIMARY KEY REFERENCES feeds(id) ON DELETE CASCADE,
  window_days INTEGER NOT NULL,
  article_count INTEGER NOT NULL,
  assessed_events INTEGER NOT NULL,
  passed_events INTEGER NOT NULL,
  irrelevant_events INTEGER NOT NULL,
  average_score REAL,
  snr REAL,
  previous_snr REAL,
  auto_adjust INTEGER NOT NULL DEFAULT 0 CHECK (auto_adjust BETWEEN -8 AND 8),
  declining BOOLEAN NOT NULL DEFAULT false,
  alerted_at TIMESTAMPTZ,
  computed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE brief_editions (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  edition_date DATE NOT NULL UNIQUE,
  window_hours INTEGER NOT NULL,
  item_count INTEGER NOT NULL,
  markdown TEXT NOT NULL,
  items JSONB NOT NULL,
  generated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  delivered_at TIMESTAMPTZ,
  delivery_error TEXT
);
