import type { EventBriefItem } from './api'

export type ReportFilters = {
  query: string
  exclude: string
  source: string
  evidence: 'all' | 'grounded' | 'unprocessed'
}

export const EMPTY_REPORT_FILTERS: ReportFilters = { query: '', exclude: '', source: '', evidence: 'all' }

export function hasReportFilters(filters: ReportFilters): boolean {
  return Boolean(filters.query.trim() || filters.exclude.trim() || filters.source || filters.evidence !== 'all')
}

export function filterReportItems(items: EventBriefItem[], filters: ReportFilters): EventBriefItem[] {
  const terms = (value: string) => value.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean)
  const required = terms(filters.query)
  const excluded = terms(filters.exclude)
  return items.filter((item) => {
    const text = [item.title, item.what_happened, ...item.source_titles, ...item.watch_names].join(' ').toLocaleLowerCase()
    return required.every((term) => text.includes(term))
      && !excluded.some((term) => text.includes(term))
      && (!filters.source || item.source_titles.some((name) => name.trim() === filters.source))
      && (filters.evidence === 'all' || (filters.evidence === 'grounded' ? item.has_grounded_summary : !item.has_grounded_summary))
  })
}
