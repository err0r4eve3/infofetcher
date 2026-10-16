import type { EventBriefItem } from './api'

export type SituationTopic = 'all' | EventBriefItem['report_section']
export type SituationPanel = 'priority' | 'recent' | 'corrections' | 'watches'
export const SITUATION_PANELS: ReadonlyArray<{ key: SituationPanel; label: string }> = [
  { key: 'priority', label: '重点事件' },
  { key: 'recent', label: '最近报道' },
  { key: 'corrections', label: '纠正与分歧' },
  { key: 'watches', label: '关注命中' }
]

export function isCorrection(item: EventBriefItem): boolean {
  return item.revision_type === 'CORRECT' || item.revision_type === 'CONTRADICT'
}

export function searchSituation(items: EventBriefItem[], query: string): EventBriefItem[] {
  const terms = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean)
  if (!terms.length) return items
  return items.filter((item) => {
    const text = [item.title, item.what_happened, ...item.source_titles, ...item.watch_names].join(' ').toLocaleLowerCase()
    return terms.every((term) => text.includes(term))
  })
}

export function situationPanelItems(items: EventBriefItem[], panel: SituationPanel): EventBriefItem[] {
  const publishedAt = (item: EventBriefItem) => Date.parse(item.published_at) || 0
  const recentFirst = (a: EventBriefItem, b: EventBriefItem) => publishedAt(b) - publishedAt(a) || a.event_id.localeCompare(b.event_id)
  if (panel === 'priority') return [...items].sort((a, b) => b.total_score - a.total_score || recentFirst(a, b))
  if (panel === 'corrections') return items.filter(isCorrection).sort(recentFirst)
  if (panel === 'watches') return items.filter((item) => item.watch_ids.length > 0).sort(recentFirst)
  return [...items].sort(recentFirst)
}

export function readHiddenPanels(value: string | null): SituationPanel[] {
  try {
    const parsed: unknown = JSON.parse(value ?? '[]')
    return Array.isArray(parsed)
      ? SITUATION_PANELS.filter(({ key }) => parsed.includes(key)).map(({ key }) => key)
      : []
  } catch { return [] }
}
