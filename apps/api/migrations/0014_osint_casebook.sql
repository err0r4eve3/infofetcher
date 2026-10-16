-- Case snapshots intentionally keep original IDs without foreign keys to the
-- reader's expiring articles/events. Reader retention must not erase evidence.
CREATE TABLE osint_cases (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  event_id UUID NOT NULL UNIQUE,
  title TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO osint_cases (event_id, title, created_at, updated_at)
SELECT c.story_id, s.representative_title, min(c.captured_at), max(c.captured_at)
FROM osint_captures c JOIN story_clusters s ON s.id = c.story_id
GROUP BY c.story_id, s.representative_title;

ALTER TABLE osint_captures DROP CONSTRAINT osint_captures_article_id_fkey;
ALTER TABLE osint_captures DROP CONSTRAINT osint_captures_story_id_fkey;
ALTER TABLE osint_captures ADD COLUMN case_id UUID REFERENCES osint_cases(id) ON DELETE RESTRICT;
UPDATE osint_captures c SET case_id = k.id FROM osint_cases k WHERE k.event_id = c.story_id;
ALTER TABLE osint_captures ALTER COLUMN case_id SET NOT NULL;
CREATE INDEX idx_osint_captures_case ON osint_captures(case_id, captured_at DESC);

CREATE TABLE osint_runs (
  id UUID PRIMARY KEY,
  case_id UUID NOT NULL REFERENCES osint_cases(id) ON DELETE RESTRICT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  created_by TEXT NOT NULL,
  report JSONB NOT NULL CHECK (jsonb_typeof(report) = 'object'),
  report_sha256 TEXT NOT NULL CHECK (report_sha256 ~ '^[0-9a-f]{64}$')
);
CREATE INDEX idx_osint_runs_case ON osint_runs(case_id, created_at DESC, id DESC);
CREATE INDEX idx_osint_runs_scan ON osint_runs(created_at DESC, id DESC);

-- Decisions append to history; they never modify the original report.
CREATE TABLE osint_reviews (
  id BIGSERIAL PRIMARY KEY,
  run_id UUID NOT NULL REFERENCES osint_runs(id) ON DELETE RESTRICT,
  target_kind TEXT NOT NULL CHECK (target_kind IN ('finding', 'fragment')),
  target_index INTEGER NOT NULL CHECK (target_index >= 0),
  verdict TEXT NOT NULL CHECK (verdict IN ('confirmed', 'rejected', 'needs_review')),
  note TEXT NOT NULL CHECK (char_length(note) BETWEEN 1 AND 2000),
  actor TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_osint_reviews_run ON osint_reviews(run_id, id DESC);

CREATE TABLE osint_entities (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  kind TEXT NOT NULL CHECK (kind IN ('person', 'organization', 'location')),
  name TEXT NOT NULL CHECK (char_length(name) BETWEEN 2 AND 120),
  aliases JSONB NOT NULL CHECK (jsonb_typeof(aliases) = 'array'),
  created_by TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE osint_mentions (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  entity_id UUID NOT NULL REFERENCES osint_entities(id) ON DELETE RESTRICT,
  run_id UUID NOT NULL REFERENCES osint_runs(id) ON DELETE RESTRICT,
  article_id UUID NOT NULL,
  matched_alias TEXT NOT NULL,
  excerpt TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('suggested', 'confirmed', 'rejected')),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(entity_id, run_id, article_id, matched_alias)
);
CREATE INDEX idx_osint_mentions_run ON osint_mentions(run_id);
CREATE INDEX idx_osint_mentions_entity ON osint_mentions(entity_id, created_at DESC);

CREATE TABLE osint_relations (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  run_id UUID NOT NULL REFERENCES osint_runs(id) ON DELETE RESTRICT,
  article_id UUID NOT NULL,
  subject_id UUID NOT NULL REFERENCES osint_entities(id) ON DELETE RESTRICT,
  object_id UUID NOT NULL REFERENCES osint_entities(id) ON DELETE RESTRICT,
  predicate TEXT NOT NULL CHECK (char_length(predicate) BETWEEN 1 AND 80),
  excerpt TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'confirmed' CHECK (status IN ('confirmed', 'rejected')),
  created_by TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CHECK (subject_id <> object_id)
);
CREATE INDEX idx_osint_relations_run ON osint_relations(run_id);

CREATE TABLE osint_entity_audit (
  id BIGSERIAL PRIMARY KEY,
  run_id UUID NOT NULL REFERENCES osint_runs(id) ON DELETE RESTRICT,
  actor TEXT NOT NULL,
  action TEXT NOT NULL,
  details JSONB NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_osint_entity_audit_run ON osint_entity_audit(run_id, id DESC);
