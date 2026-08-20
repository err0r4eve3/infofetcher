CREATE TABLE event_digests (
  story_id UUID PRIMARY KEY REFERENCES story_clusters(id) ON DELETE CASCADE,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (
    status IN ('pending', 'running', 'done', 'failed', 'skipped')
  ),
  topic TEXT,
  message TEXT,
  verification_status TEXT CHECK (
    verification_status IS NULL OR verification_status IN (
      'corroborated', 'developing', 'disputed', 'corrected', 'insufficient'
    )
  ),
  verification_label TEXT,
  verification_score INTEGER CHECK (
    verification_score IS NULL OR verification_score BETWEEN 0 AND 100
  ),
  verification_reason TEXT,
  revision_type TEXT CHECK (
    revision_type IS NULL OR revision_type IN ('ADD', 'UPDATE', 'CORRECT', 'CONTRADICT', 'CONFIRM')
  ),
  model TEXT,
  prompt_version TEXT NOT NULL DEFAULT 'event-wire-agent-v1',
  source_hash TEXT,
  source_article_ids UUID[] NOT NULL DEFAULT '{}',
  source_titles TEXT[] NOT NULL DEFAULT '{}',
  source_count INTEGER NOT NULL DEFAULT 0 CHECK (source_count BETWEEN 0 AND 5),
  cited_source_count INTEGER NOT NULL DEFAULT 0 CHECK (
    cited_source_count BETWEEN 0 AND source_count
  ),
  citations JSONB NOT NULL DEFAULT '[]'::jsonb,
  disagreements JSONB NOT NULL DEFAULT '[]'::jsonb,
  agent_state JSONB NOT NULL DEFAULT '{"phase":"pending","steps":[]}'::jsonb,
  published_at TIMESTAMPTZ,
  error TEXT,
  completed_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CHECK (
    status <> 'done'
    OR (
      message IS NOT NULL
      AND verification_status IS NOT NULL
      AND source_count = cardinality(source_article_ids)
      AND completed_at IS NOT NULL
    )
  )
);

CREATE INDEX idx_event_digests_published
  ON event_digests(published_at DESC, verification_score DESC)
  WHERE status = 'done';
CREATE INDEX idx_event_digests_source_articles
  ON event_digests USING gin (source_article_ids);

CREATE TABLE event_agent_runs (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  story_id UUID NOT NULL REFERENCES story_clusters(id) ON DELETE CASCADE,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (
    status IN ('pending', 'running', 'done', 'failed')
  ),
  attempts INTEGER NOT NULL DEFAULT 0,
  max_attempts INTEGER NOT NULL DEFAULT 4,
  available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  locked_at TIMESTAMPTZ,
  run_token UUID,
  phase TEXT NOT NULL DEFAULT 'pending',
  error TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  completed_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX idx_event_agent_runs_active_story
  ON event_agent_runs(story_id)
  WHERE status IN ('pending', 'running');
CREATE INDEX idx_event_agent_runs_due
  ON event_agent_runs(status, available_at, created_at);
