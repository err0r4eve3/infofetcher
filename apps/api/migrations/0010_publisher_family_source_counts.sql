WITH publisher_counts AS (
  SELECT a.story_id,
         COUNT(DISTINCT lower(regexp_replace(
           COALESCE(
             substring(COALESCE(NULLIF(f.site_url, ''), f.feed_url)
               from '^[a-zA-Z][a-zA-Z0-9+.-]*://([^/:?#]+)'),
             COALESCE(NULLIF(f.site_url, ''), f.feed_url)
           ),
           '^www[.]', ''
         )))::integer AS source_count
  FROM articles a
  JOIN feeds f ON f.id = a.feed_id
  WHERE a.story_id IS NOT NULL
  GROUP BY a.story_id
)
UPDATE story_clusters sc
SET source_count = publisher_counts.source_count,
    updated_at = now()
FROM publisher_counts
WHERE sc.id = publisher_counts.story_id
  AND sc.source_count IS DISTINCT FROM publisher_counts.source_count;
