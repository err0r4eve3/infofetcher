WITH classified AS (
  SELECT er.id,
         a.title,
         CASE
           WHEN lower(a.title) ~ '(correction|corrected|修正|更正|纠正)'
             THEN 'CORRECT'
           WHEN lower(a.title) ~ '(fact[ -]?check|false (claim|report)|(denies|denied) (the )?(claim|report|allegation)|refutes? (claim|report)|debunks? (claim|report)|官方否认|否认.{0,12}(报道|指控|说法|传闻)|辟谣|(报道|消息|说法|传闻)不实)'
             THEN 'CONTRADICT'
           WHEN er.revision_type = 'ADD' THEN 'ADD'
           WHEN er.source_count >= 2 THEN 'CONFIRM'
           ELSE 'UPDATE'
         END AS revision_type
  FROM event_revisions er
  JOIN articles a ON a.id = er.article_id AND a.story_id = er.story_id
)
UPDATE event_revisions er
SET title = classified.title,
    revision_type = classified.revision_type,
    occurred_at = CASE
      WHEN classified.revision_type IN ('CORRECT', 'CONTRADICT')
       AND er.revision_type IS DISTINCT FROM classified.revision_type
        THEN clock_timestamp()
      ELSE er.occurred_at
    END,
    created_at = CASE
      WHEN classified.revision_type IN ('CORRECT', 'CONTRADICT')
       AND er.revision_type IS DISTINCT FROM classified.revision_type
        THEN clock_timestamp()
      ELSE er.created_at
    END
FROM classified
WHERE er.id = classified.id
  AND (
    er.title IS DISTINCT FROM classified.title
    OR er.revision_type IS DISTINCT FROM classified.revision_type
  );
