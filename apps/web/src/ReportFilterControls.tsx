import type { EventBriefItem } from './api'
import { EMPTY_REPORT_FILTERS, hasReportFilters, type ReportFilters } from './reportFilters'
import { sourceNames } from './coverageModel'
import './sourceFilters.css'

export function ReportFilterControls({ items, value, onChange }: {
  items: EventBriefItem[]
  value: ReportFilters
  onChange: (value: ReportFilters) => void
}) {
  const sources = sourceNames(items.flatMap((item) => item.source_titles)).sort((a, b) => a.localeCompare(b))
  // Keep a selected source visible when a refreshed report no longer contains it.
  if (value.source && !sources.includes(value.source)) sources.unshift(value.source)
  return <div className="source-filter-controls" role="group" aria-label="事件组合筛选">
    <label>关键词<input type="search" value={value.query} placeholder="空格分隔，全部匹配" onChange={(event) => onChange({ ...value, query: event.target.value })} /></label>
    <label>排除词<input type="search" value={value.exclude} placeholder="空格分隔，任一排除" onChange={(event) => onChange({ ...value, exclude: event.target.value })} /></label>
    <label>订阅源<select aria-label="订阅源" value={value.source} onChange={(event) => onChange({ ...value, source: event.target.value })}>
      <option value="">全部订阅源</option>{sources.map((source) => <option key={source} value={source}>{source}</option>)}
    </select></label>
    <label>摘要材料<select aria-label="摘要材料" value={value.evidence} onChange={(event) => onChange({ ...value, evidence: event.target.value as ReportFilters['evidence'] })}>
      <option value="all">全部材料</option><option value="grounded">有材料支撑摘要</option><option value="unprocessed">暂无材料支撑摘要</option>
    </select></label>
    {hasReportFilters(value) && <button type="button" className="secondary-button" onClick={() => onChange(EMPTY_REPORT_FILTERS)}>重置条件</button>}
  </div>
}
