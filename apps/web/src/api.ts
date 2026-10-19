import type { ReportSectionKey } from './reportSections'

export type Folder = {
  id: string
  title: string
  sort_order: number
  created_at: string
}

export type Feed = {
  id: string
  folder_id: string | null
  title: string
  site_url: string | null
  feed_url: string
  hub_adapter: string | null
  hub_path: string | null
  source_type: string
  language: string | null
  region: string | null
  quality_score: number
  feedback_weight: number
  muted: boolean
  last_fetched_at: string | null
  last_success_at: string | null
  next_fetch_at: string | null
  consecutive_failures: number
  last_error: string | null
  article_count: number
  unread_count: number
  editorial_profile: EditorialProfile | null
  quality_stats: FeedQualityStats | null
}

export type EditorialProfile = 'world-news' | 'tech-research' | 'long-read'

export type FeedQualityStats = {
  snr: number | null
  average_score: number | null
  passed_events: number
  assessed_events: number
  auto_adjust: number
  declining: boolean
}

export type ArticleListItem = {
  id: string
  feed_id: string
  feed_title: string
  title: string
  url: string
  author: string | null
  excerpt: string
  language: string | null
  quality_score: number
  priority_score: number
  priority_reasons: string[]
  story_id: string | null
  story_source_count: number
  published_at: string | null
  fetched_at: string
  is_read: boolean
  is_favorite: boolean
  has_summary: boolean
}

export type Summary = {
  article_id: string
  status: 'pending' | 'running' | 'done' | 'failed'
  model: string | null
  schema_version: number
  prompt_version: string
  method: string
  scope: 'article' | 'event_snapshot'
  source_count: number
  cited_source_count: number
  source_article_ids: string[]
  input_hash: string | null
  summary: string | null
  bullets: string[]
  keywords: string[]
  confidence: string | null
  confidence_reason: string | null
  grounding_score: number | null
  corroboration: 'single' | 'multi' | 'divergent' | null
  disagreements: string[]
  citations: Array<{
    excerpt: string
    claim_id: string | null
    evidence_id: string | null
    article_id: string | null
    source_title: string | null
    article_title: string | null
    url: string | null
    verified: boolean
  }>
  error: string | null
  updated_at: string
}

export type KeywordRule = {
  id: string
  name: string
  keywords: string[]
  match_title: boolean
  match_content: boolean
  case_sensitive: boolean
  auto_summary: boolean
  telegram_enabled: boolean
  enabled: boolean
  feed_ids: string[]
  folder_ids: string[]
  match_count: number
  created_at: string
  updated_at: string
}

export type KeywordRuleRequest = {
  name: string
  keywords: string[]
  match_title: boolean
  match_content: boolean
  case_sensitive: boolean
  auto_summary: boolean
  telegram_enabled: boolean
  enabled: boolean
  feed_ids: string[]
  folder_ids: string[]
}

export type KeywordMatch = {
  id: string
  rule_id: string
  rule_name: string
  article_id: string
  feed_id: string
  feed_title: string
  article_title: string
  article_url: string
  excerpt: string
  published_at: string | null
  fetched_at: string
  matched_keywords: string[]
  summary_status: string | null
  telegram_status: 'not_sent' | 'sent' | 'failed'
  telegram_error: string | null
  created_at: string
}

export type KeywordScanResponse = {
  scanned: number
  matched: number
  new_matches: number
  queued_summaries: number
  telegram_sent: number
  telegram_failed: number
}

export type KeywordActionResponse = {
  queued_summaries: number
  telegram_sent: number
  telegram_failed: number
}

export type ArticleDetail = {
  id: string
  feed_id: string
  feed_title: string
  title: string
  url: string
  author: string | null
  content: string
  extracted_content: string
  extraction_error: string | null
  language: string | null
  quality_score: number
  story_id: string | null
  story_source_count: number
  published_at: string | null
  fetched_at: string
  is_read: boolean
  is_favorite: boolean
  summary: Summary | null
  translation: ArticleTranslation | null
}

export type ArticleTranslation = {
  article_id: string
  target_language: 'zh-Hans' | 'en'
  status: 'running' | 'done' | 'failed'
  model: string
  source_hash: string
  prompt_version: string
  translated_title: string | null
  translated_content: string | null
  error: string | null
  updated_at: string
}

export type SelectionTranslation = {
  translation: string
  target_language: 'zh-Hans' | 'en'
  model: string
}

export type Metrics = {
  public_url: string
  feeds: number
  articles: number
  unread: number
  summaries: number
  stories: number
  pending_jobs: number
  failed_jobs: number
  unhealthy_feeds: number
}

export type StoryCluster = {
  id: string
  representative_title: string
  source_count: number
  quality_score: number
  first_published_at: string | null
  last_published_at: string | null
  articles: ArticleListItem[]
}

export type Monitor = {
  id: string
  name: string
  query: string
  min_quality: number
  unread_only: boolean
  enabled: boolean
  created_at: string
  updated_at: string
}

export type MonitorRequest = {
  name: string
  query: string
  min_quality: number
  unread_only: boolean
  enabled: boolean
}

export type EventBriefItem = {
  event_id: string
  report_section: ReportSectionKey
  revision_type: 'ADD' | 'UPDATE' | 'CORRECT' | 'CONTRADICT' | 'CONFIRM'
  title: string
  what_happened: string
  why_it_matters: string
  why_now: string
  confidence_label: string
  confidence_score: number
  source_quality_score: number
  impact_score: number
  novelty_score: number
  urgency_score: number
  relevance_score: number
  total_score: number
  source_count: number
  independent_source_count: number
  source_titles: string[]
  watch_ids: string[]
  watch_names: string[]
  representative_article_id: string
  published_at: string
  update_count: number
  has_grounded_summary: boolean
  feedback: 'more' | 'less' | 'known' | 'irrelevant' | 'bad_cluster' | null
  editorial_profile: EditorialProfile | null
  editorial_score: number | null
  editorial_tags: string[]
  history_note: string | null
  history_refs: Array<{ event_id: string; title: string; digest_date: string }>
  community_note: string | null
  takeaway: string | null
}

export type BriefEditionList = {
  feed_path: string | null
  editions: Array<{ edition_date: string; generated_at: string; item_count: number }>
}

export type NowBrief = {
  generated_at: string
  window_hours: number
  mode: 'report' | 'brief'
  budget_minutes: number | null
  stats: {
    candidate_events: number
    eligible_events: number
    selected_events: number
    independent_sources: number
    watch_matches: number
    assessed_events?: number
  }
  items: EventBriefItem[]
}

export type WireItem = {
  event_id: string
  topic: string
  message: string
  verification_status: 'corroborated' | 'developing' | 'disputed' | 'corrected' | 'insufficient'
  verification_label: string
  verification_score: number
  verification_reason: string
  revision_type: EventBriefItem['revision_type']
  source_count: number
  cited_source_count: number
  source_titles: string[]
  citations: Summary['citations']
  disagreements: string[]
  published_at: string
  updated_at: string
}

export type WireResponse = {
  generated_at: string
  window_hours: number
  stats: {
    published: number
    pending: number
    running: number
    failed: number
    skipped: number
  }
  items: WireItem[]
}

export type EventDetail = {
  event: EventBriefItem
  evidence_scope?: {
    total_articles: number
    returned_articles: number
    duplicate_groups: number
    feeds_with_failures: number
    feeds_never_succeeded: number
  }
  timeline: Array<{
    id: string
    revision_type: EventBriefItem['revision_type']
    title: string
    source_count: number
    occurred_at: string
    article_id: string | null
  }>
  evidence: Array<{
    article_id: string
    source_title: string
    title: string
    url: string
    excerpt: string
    quality_score: number
    published_at: string
    reported_published_at?: string | null
    fetched_at?: string
    provenance?: {
      link_host: string | null
      feed_host: string | null
      link_is_aggregator: boolean
      duplicate_group: number | null
    }
    collection?: {
      feed_id: string
      last_success_at: string | null
      consecutive_failures: number
    }
  }>
}

export type Watch = {
  id: string
  name: string
  query: string
  min_impact: number
  notification_mode: 'off' | 'brief' | 'urgent'
  enabled: boolean
  created_at: string
  updated_at: string
}

export type WatchRequest = {
  name: string
  query: string
  min_impact: number
  notification_mode: Watch['notification_mode']
  enabled: boolean
}

// Tracked-event watches end with " · <event id prefix>" to satisfy the unique
// name constraint; the suffix is not meaningful to readers.
export function watchDisplayName(name: string): string {
  return name.replace(/ · [0-9a-f]{6}$/, '')
}

export class ApiError extends Error {
  status: number

  constructor(status: number, message: string) {
    super(message)
    this.name = 'ApiError'
    this.status = status
  }
}

export async function api<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers)
  if (init.body && !headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json')
  }
  const response = await fetch(path, {
    ...init,
    credentials: 'include',
    headers
  })
  if (!response.ok) {
    let message = `${response.status} ${response.statusText}`
    try {
      const body = (await response.json()) as { error?: string }
      message = body.error ?? message
    } catch {
      // Keep HTTP status fallback.
    }
    throw new ApiError(response.status, message)
  }
  if (response.status === 204) {
    return undefined as T
  }
  return response.json() as Promise<T>
}

const PREFETCH_TTL_MS = 15_000
const prefetches = new Map<string, { request: Promise<unknown>; startedAt: number }>()

// Starts a GET before the view that needs it has loaded; failures resolve to null so the view refetches.
export function prefetch(path: string) {
  if (prefetches.has(path)) return
  prefetches.set(path, { request: api(path).catch(() => null), startedAt: Date.now() })
}

export function takePrefetched<T>(path: string): Promise<T | null> | null {
  const entry = prefetches.get(path)
  if (!entry) return null
  // Kept for a short window so React StrictMode's remount adopts the same response.
  if (Date.now() - entry.startedAt > PREFETCH_TTL_MS) {
    prefetches.delete(path)
    return null
  }
  return entry.request as Promise<T | null>
}

export function formatDate(value: string | null): string {
  if (!value) return '未知时间'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return '未知时间'
  return new Intl.DateTimeFormat('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit'
  }).format(date)
}
