import type { EventBriefItem, EventDetail } from './api'

export type CoverageFilter = 'all' | 'multiple' | 'single'
export type EvidenceOrder = 'latest' | 'quality'
export type Evidence = EventDetail['evidence'][number]

// Use the server's source-family count, never the number of articles or feed titles.
export function matchesCoverage(item: Pick<EventBriefItem, 'independent_source_count'>, filter: CoverageFilter): boolean {
  if (filter === 'multiple') return item.independent_source_count >= 2
  if (filter === 'single') return item.independent_source_count === 1
  return true
}

export function coverageLabel(count: number): string {
  return count > 0 ? `${count} 个来源组` : '来源待确认'
}

export function sourceNames(names: string[] | null | undefined): string[] {
  return [...new Set((names ?? []).map((name) => name.trim()).filter(Boolean))]
}

// Feed titles carry a section or tagline: "Section | Brand" puts the brand last,
// "Brand - Tagline" puts it first.
export function shortSourceName(name: string): string {
  const trimmed = name.trim()
  const piped = trimmed.split(/\s+\|\s+/)
  const brand = piped.length > 1 ? piped[piped.length - 1] : trimmed.split(/\s+[-–—]\s+/)[0]
  return brand.length >= 2 ? brand : trimmed
}

// A missing publication date must remain unknown even when the API supplies a sorting fallback.
export function evidenceTimestamp(item: Evidence): { kind: 'published' | 'collected' | 'legacy'; value: string | undefined } {
  if (item.reported_published_at === undefined) return { kind: 'legacy', value: item.published_at }
  return item.reported_published_at
    ? { kind: 'published', value: item.reported_published_at }
    : { kind: 'collected', value: item.fetched_at }
}

export function evidenceForDisplay(evidence: Evidence[], source: string, order: EvidenceOrder): Evidence[] {
  const time = (item: Evidence) => {
    const timestamp = Date.parse(evidenceTimestamp(item).value ?? '')
    return Number.isFinite(timestamp) ? timestamp : 0
  }
  return evidence.filter((item) => !source || item.source_title.trim() === source).sort((a, b) => (
    (order === 'quality' ? b.quality_score - a.quality_score : 0)
    || time(b) - time(a)
    || a.article_id.localeCompare(b.article_id)
  ))
}
