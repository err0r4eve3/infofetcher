import { useEffect, useMemo, useRef, useState } from 'react'
import { ArrowRight, Bell, ChartNoAxesCombined, Cpu, FileText, Globe2, Layers3, LayoutGrid, Search, ShieldCheck, X } from 'lucide-react'
import type { EventBriefItem, NowBrief } from './api'
import { formatDate, watchDisplayName } from './api'
import { isCorrection, readHiddenPanels, SITUATION_PANELS, situationPanelItems, type SituationPanel, type SituationTopic } from './situation'
import './situation.css'
import { ReportFilterControls } from './ReportFilterControls'
import { EMPTY_REPORT_FILTERS, filterReportItems, hasReportFilters, type ReportFilters } from './reportFilters'

const TOPICS = [
  { key: 'all', label: '全部', icon: LayoutGrid },
  { key: 'world', label: '全球', icon: Globe2 },
  { key: 'technology', label: '科技', icon: Cpu },
  { key: 'security', label: '安全', icon: ShieldCheck },
  { key: 'business', label: '商业', icon: ChartNoAxesCombined },
  { key: 'general', label: '其他', icon: Layers3 }
] as const
const REVISIONS = { ADD: '新增', UPDATE: '更新', CONFIRM: '确认', CORRECT: '纠正', CONTRADICT: '分歧' }

export function SituationBoard({ brief, userName, filters, onFiltersChange, onOpenEvent }: {
  brief: NowBrief
  userName: string
  filters: ReportFilters
  onFiltersChange: (filters: ReportFilters) => void
  onOpenEvent: (eventId: string) => void
}) {
  const [topic, setTopic] = useState<SituationTopic>('all')
  const storageKey = `infofetcher:situation-panels:v1:${userName}`
  const [hidden, setHidden] = useState<SituationPanel[]>(() => {
    try { return readHiddenPanels(window.localStorage.getItem(storageKey)) } catch { return [] }
  })
  const [storageError, setStorageError] = useState(false)
  const settingsRef = useRef<HTMLDetailsElement>(null)
  useEffect(() => {
    try {
      window.localStorage.setItem(storageKey, JSON.stringify(hidden))
      setStorageError(false)
    } catch { setStorageError(true) }
  }, [hidden, storageKey])
  const searched = useMemo(() => filterReportItems(brief.items, filters), [brief.items, filters])
  const filtered = useMemo(() => searched.filter((item) => topic === 'all' || item.report_section === topic), [searched, topic])
  const panels = useMemo(() => SITUATION_PANELS.map((panel) => ({
    ...panel, items: situationPanelItems(filtered, panel.key)
  })), [filtered])
  const stats = [
    { label: '事件', count: filtered.length },
    { label: '多源报道', count: filtered.filter((item) => item.independent_source_count >= 2).length },
    { label: '纠正与分歧', count: filtered.filter(isCorrection).length },
    { label: '命中关注', count: filtered.filter((item) => item.watch_ids.length > 0).length }
  ]

  function hidePanel(key: SituationPanel) {
    setHidden((current) => [...current, key])
    settingsRef.current?.querySelector('summary')?.focus()
  }

  return <section className="situation-board" aria-label="态势看板">
    <aside className="situation-topics">
      <h2>主题</h2>
      <nav aria-label="态势主题">
        {TOPICS.map(({ key, label, icon: Icon }) => <button type="button" key={key} aria-pressed={topic === key} onClick={() => setTopic(key)}>
          <Icon size={18} aria-hidden="true" /><span>{label}</span>
          <span>{key === 'all' ? searched.length : searched.filter((item) => item.report_section === key).length}</span>
        </button>)}
      </nav>
      <div className="situation-scope"><FileText size={16} aria-hidden="true" /><strong>本期报告</strong><span>近 {brief.window_hours} 小时</span></div>
    </aside>
    <div className="situation-content">
      <div className="situation-overview">
        <header className="situation-toolbar">
          <h2>态势</h2>
          <details className="situation-settings" ref={settingsRef} onKeyDown={(event) => {
            if (event.key === 'Escape') { event.currentTarget.open = false; event.currentTarget.querySelector('summary')?.focus() }
          }} onBlur={(event) => {
            if (!event.currentTarget.contains(event.relatedTarget)) event.currentTarget.open = false
          }}>
            <summary><LayoutGrid size={16} aria-hidden="true" />面板</summary>
            <div className="situation-settings-menu">
              {SITUATION_PANELS.map(({ key, label }) => <label key={key}>
                <input type="checkbox" checked={!hidden.includes(key)} onChange={(event) => setHidden((current) => event.target.checked ? current.filter((value) => value !== key) : [...current, key])} />{label}
              </label>)}
              <button type="button" onClick={() => setHidden([])}>恢复全部面板</button>
              <small>{storageError ? '偏好暂未保存，下次打开会恢复默认。' : '面板偏好保存在此浏览器。'}</small>
            </div>
          </details>
        </header>
        <ReportFilterControls items={brief.items} value={filters} onChange={onFiltersChange} />
        <dl className="situation-stats" aria-label="当前筛选统计">
          {stats.map(({ label, count }) => <div key={label}><dt>{label}</dt><dd>{count}</dd></div>)}
        </dl>
      </div>
      <div className="situation-scope-note" role="status">
        <span>本期报告内统计 · {filtered.length} / {brief.items.length} 项</span>
        {(topic !== 'all' || hasReportFilters(filters)) && <button type="button" onClick={() => { setTopic('all'); onFiltersChange(EMPTY_REPORT_FILTERS) }}>清除筛选</button>}
      </div>
      {!filtered.length ? <div className="situation-empty" role="status">
        <Search size={24} aria-hidden="true" /><p>{brief.items.length ? '没有匹配的事件' : '本期报告暂无事件'}</p>
        {brief.items.length > 0 && <button type="button" onClick={() => { setTopic('all'); onFiltersChange(EMPTY_REPORT_FILTERS) }}>清除筛选</button>}
      </div> : hidden.length === SITUATION_PANELS.length ? <div className="situation-empty">
        <LayoutGrid size={24} aria-hidden="true" /><p>所有面板已隐藏</p><button type="button" onClick={() => setHidden([])}>恢复全部面板</button>
      </div> : <div className="situation-panels">
        {panels.filter((panel) => !hidden.includes(panel.key)).map((panel) => <SignalPanel
          key={`${panel.key}:${topic}:${JSON.stringify(filters)}:${brief.window_hours}`} panel={panel.key} label={panel.label} items={panel.items}
          onHide={() => hidePanel(panel.key)} onOpenEvent={onOpenEvent}
        />)}
      </div>}
      <p className="situation-footnote">多源报道按去重订阅源数统计，不代表独立核验。<time dateTime={brief.generated_at}>{formatDate(brief.generated_at)} 生成</time></p>
    </div>
  </section>
}

function SignalPanel({ panel, label, items, onHide, onOpenEvent }: {
  panel: SituationPanel
  label: string
  items: EventBriefItem[]
  onHide: () => void
  onOpenEvent: (id: string) => void
}) {
  const [expanded, setExpanded] = useState(false)
  const limit = panel === 'priority' || panel === 'recent' ? 4 : 2
  const visible = expanded ? items : items.slice(0, limit)
  return <section className={`situation-panel situation-panel-${panel}`} aria-labelledby={`situation-${panel}`}>
    <header><h3 id={`situation-${panel}`}>{label}</h3><span>{items.length}</span><button type="button" aria-label={`隐藏${label}`} title={`隐藏${label}`} onClick={onHide}><X size={15} aria-hidden="true" /></button></header>
    {!items.length ? <p className="situation-panel-empty">{panel === 'watches' ? '当前筛选没有关注命中' : '当前筛选没有纠正或分歧'}</p> : <ol>
      {visible.map((item, index) => <li key={item.event_id}>
        <button type="button" className={`situation-event${isCorrection(item) ? ' has-correction' : ''}`} onClick={() => onOpenEvent(item.event_id)}>
          {panel === 'priority' ? <span className="situation-rank" aria-hidden="true">{String(index + 1).padStart(2, '0')}</span>
            : panel === 'watches' ? <Bell size={17} aria-hidden="true" />
              : <time dateTime={item.published_at} title={formatDate(item.published_at)}>{new Date(item.published_at).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}</time>}
          <span className="situation-event-body">
            <strong>{item.title}</strong>
            <span className="situation-excerpt">{panel === 'watches' ? `命中关注：${item.watch_names.map(watchDisplayName).join('、') || '已匹配关注规则'}` : panel === 'priority' ? item.what_happened : item.why_now}</span>
            <span className="situation-event-meta">
              <span className={`situation-revision type-${item.revision_type}${isCorrection(item) ? ' correction' : ''}`}>{REVISIONS[item.revision_type]}</span>
              {panel === 'priority' ? <span>{item.independent_source_count} 个去重订阅源 · {item.source_titles.slice(0, 3).join(' / ') || '来源待补全'}</span> : <span>{item.source_titles[0] || '来源待补全'}</span>}
            </span>
          </span>
          <ArrowRight size={15} aria-hidden="true" />
        </button>
      </li>)}
    </ol>}
    {items.length > limit && <button type="button" className="situation-expand" aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}>{expanded ? '收起' : `展开其余 ${items.length - limit} 项`}</button>}
  </section>
}
