CREATE TABLE keyword_rules (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  name TEXT NOT NULL,
  keywords TEXT[] NOT NULL DEFAULT '{}',
  match_title BOOLEAN NOT NULL DEFAULT true,
  match_content BOOLEAN NOT NULL DEFAULT true,
  case_sensitive BOOLEAN NOT NULL DEFAULT false,
  auto_summary BOOLEAN NOT NULL DEFAULT true,
  telegram_enabled BOOLEAN NOT NULL DEFAULT false,
  enabled BOOLEAN NOT NULL DEFAULT true,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE keyword_rule_feeds (
  rule_id UUID NOT NULL REFERENCES keyword_rules(id) ON DELETE CASCADE,
  feed_id UUID NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
  PRIMARY KEY(rule_id, feed_id)
);

CREATE TABLE keyword_rule_folders (
  rule_id UUID NOT NULL REFERENCES keyword_rules(id) ON DELETE CASCADE,
  folder_id UUID NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
  PRIMARY KEY(rule_id, folder_id)
);

CREATE TABLE keyword_matches (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  rule_id UUID NOT NULL REFERENCES keyword_rules(id) ON DELETE CASCADE,
  article_id UUID NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
  matched_keywords TEXT[] NOT NULL DEFAULT '{}',
  summary_queued BOOLEAN NOT NULL DEFAULT false,
  telegram_status TEXT NOT NULL DEFAULT 'not_sent' CHECK (telegram_status IN ('not_sent', 'sent', 'failed')),
  telegram_error TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(rule_id, article_id)
);

CREATE INDEX idx_keyword_matches_rule_created ON keyword_matches(rule_id, created_at DESC);
CREATE INDEX idx_keyword_matches_article ON keyword_matches(article_id);
