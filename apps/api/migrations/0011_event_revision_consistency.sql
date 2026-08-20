UPDATE event_revisions er
SET story_id = a.story_id,
    title = a.title
FROM articles a
WHERE er.article_id = a.id
  AND a.story_id IS NOT NULL
  AND (
    er.story_id IS DISTINCT FROM a.story_id
    OR er.title IS DISTINCT FROM a.title
  );

WITH revision_counts AS (
  SELECT er.id,
         GREATEST(COUNT(DISTINCT lower(regexp_replace(
           COALESCE(
             substring(COALESCE(NULLIF(f.site_url, ''), f.feed_url)
               from '^[a-zA-Z][a-zA-Z0-9+.-]*://([^/:?#]+)'),
             COALESCE(NULLIF(f.site_url, ''), f.feed_url)
           ),
           '^www[.]', ''
         ))), 1)::integer AS source_count
  FROM event_revisions er
  LEFT JOIN articles a
    ON a.story_id = er.story_id
   AND COALESCE(a.published_at, a.fetched_at) <= er.occurred_at
  LEFT JOIN feeds f ON f.id = a.feed_id
  GROUP BY er.id
)
UPDATE event_revisions er
SET source_count = revision_counts.source_count,
    revision_type = CASE
      WHEN er.revision_type IN ('ADD', 'CORRECT', 'CONTRADICT')
        THEN er.revision_type
      WHEN revision_counts.source_count >= 2 THEN 'CONFIRM'
      ELSE 'UPDATE'
    END
FROM revision_counts
WHERE er.id = revision_counts.id
  AND (
    er.source_count IS DISTINCT FROM revision_counts.source_count
    OR er.revision_type IS DISTINCT FROM CASE
      WHEN er.revision_type IN ('ADD', 'CORRECT', 'CONTRADICT')
        THEN er.revision_type
      WHEN revision_counts.source_count >= 2 THEN 'CONFIRM'
      ELSE 'UPDATE'
    END
  );
