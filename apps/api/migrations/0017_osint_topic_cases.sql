-- Topic cases start from a user query instead of a clustered event. Their
-- event_id is a synthetic key that never matches story_clusters.
ALTER TABLE osint_cases ADD COLUMN origin TEXT NOT NULL DEFAULT 'event'
  CHECK (origin IN ('event', 'topic'));
