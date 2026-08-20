ALTER TABLE summaries
  ADD COLUMN schema_version INTEGER NOT NULL DEFAULT 1,
  ADD COLUMN prompt_version TEXT NOT NULL DEFAULT 'legacy',
  ADD COLUMN method TEXT NOT NULL DEFAULT 'legacy',
  ADD COLUMN scope TEXT NOT NULL DEFAULT 'article',
  ADD COLUMN source_count INTEGER NOT NULL DEFAULT 1,
  ADD COLUMN cited_source_count INTEGER NOT NULL DEFAULT 0,
  ADD COLUMN run_token UUID,
  ADD COLUMN source_article_ids UUID[] NOT NULL DEFAULT '{}',
  ADD COLUMN input_hash TEXT,
  ADD COLUMN grounding_score INTEGER,
  ADD COLUMN corroboration TEXT,
  ADD COLUMN disagreements JSONB NOT NULL DEFAULT '[]'::jsonb;

ALTER TABLE worker_jobs
  ADD COLUMN run_token UUID;

UPDATE summaries
SET source_article_ids = ARRAY[article_id]
WHERE source_article_ids = '{}';

ALTER TABLE summaries
  ADD CONSTRAINT summaries_source_count_check CHECK (source_count BETWEEN 1 AND 5),
  ADD CONSTRAINT summaries_cited_source_count_check
    CHECK (cited_source_count BETWEEN 0 AND source_count),
  ADD CONSTRAINT summaries_done_source_snapshot_check
    CHECK (
      status <> 'done'
      OR method = 'legacy'
      OR source_count = cardinality(source_article_ids)
    ),
  ADD CONSTRAINT summaries_grounding_score_check
    CHECK (grounding_score IS NULL OR grounding_score BETWEEN 0 AND 100),
  ADD CONSTRAINT summaries_scope_check CHECK (scope IN ('article', 'event_snapshot'));

CREATE INDEX idx_summaries_source_article_ids
  ON summaries USING gin (source_article_ids);

WITH ranked_active_jobs AS (
  SELECT id,
         row_number() OVER (
           PARTITION BY article_id
           ORDER BY created_at, id
         ) AS duplicate_rank
  FROM summary_jobs
  WHERE article_id IS NOT NULL AND status IN ('pending', 'running')
)
DELETE FROM summary_jobs job
USING ranked_active_jobs ranked
WHERE job.id = ranked.id AND ranked.duplicate_rank > 1;

CREATE UNIQUE INDEX idx_summary_jobs_active_article
  ON summary_jobs(article_id)
  WHERE article_id IS NOT NULL AND status IN ('pending', 'running');
