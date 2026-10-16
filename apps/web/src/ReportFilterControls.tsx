import { useId, useState } from 'react'
import { Search, SlidersHorizontal } from 'lucide-react'
import type { EventBriefItem } from './api'
import { EMPTY_REPORT_FILTERS, hasReportFilters, type ReportFilters } from './reportFilters'
import { sourceNames } from './coverageModel'
import './sourceFilters.css'

export function ReportFilterControls({ items, value, onChange }: {
  items: EventBriefItem[]
  value: ReportFilters
  onChange: (value: ReportFilters) => void
}) {
  const advancedId = useId()
  const advancedCount = [value.exclude.trim(), value.source, value.evidence !== 'all'].filter(Boolean).length
  // Start expanded when a restored URL already carries advanced conditions.
  const [expanded, setExpanded] = useState(advancedCount > 0)
  const sources = sourceNames(items.flatMap((item) => item.source_titles)).sort((a, b) => a.localeCompare(b))
  // Keep a selected source visible when a refreshed report no longer contains it.
  if (value.source && !sources.includes(value.source)) sources.unshift(value.source)
  return <div className="report-filter-bar" role="group" aria-label="事件组合筛选">
    <div className="report-filter-primary">
      <label className="report-filter-query">
        <Search size={16} aria-hidden="true" />
        <span className="visually-hidden">关键词</span>
        <input type="search" value={value.query} placeholder="关键词，空格分隔" onChange={(event) => onChange({ ...value, query: event.target.value })} />
      </label>
      <button type="button" className="report-filter-toggle" aria-expanded={expanded} aria-controls={advancedId} onClick={() => setExpanded((current) => !current)}>
        <SlidersHorizontal size={16} aria-hidden="true" />筛选{advancedCount > 0 && <b>{advancedCount}</b>}
      </button>
      {hasReportFilters(value) && <button type="button" className="report-filter-reset" onClick={() => onChange(EMPTY_REPORT_FILTERS)}>重置</button>}
    </div>
    <div id={advancedId} className="source-filter-controls" hidden={!expanded}>
      <label>排除词<input type="search" value={value.exclude} placeholder="空格分隔，任一排除" onChange={(event) => onChange({ ...value, exclude: event.target.value })} /></label>
      <label>订阅源<select aria-label="订阅源" value={value.source} onChange={(event) => onChange({ ...value, source: event.target.value })}>
        <option value="">全部订阅源</option>{sources.map((source) => <option key={source} value={source}>{source}</option>)}
      </select></label>
      <label>摘要材料<select aria-label="摘要材料" value={value.evidence} onChange={(event) => onChange({ ...value, evidence: event.target.value as ReportFilters['evidence'] })}>
        <option value="all">全部材料</option><option value="grounded">有材料支撑摘要</option><option value="unprocessed">暂无材料支撑摘要</option>
      </select></label>
    </div>
  </div>
}
