import {
  ArrowRight,
  BellRing,
  Bookmark,
  BookOpen,
  CheckCircle2,
  Clock3,
  Compass,
  Eye,
  FileSearch,
  Library,
  ListChecks,
  Plus,
  Radio,
  RefreshCw,
  Search,
  Settings,
  ShieldCheck,
  Sparkles,
  Target,
  Trash2,
  TrendingUp,
  XCircle
} from 'lucide-react'
import { FormEvent, useCallback, useEffect, useMemo, useState } from 'react'
import {
  api,
  EventBriefItem,
  EventDetail,
  formatDate,
  NowBrief,
  WireResponse,
  Watch,
  WatchRequest
} from './api'

type RadarView = 'now' | 'wire' | 'watches'
type BriefWorkflow = 'pending' | 'saved' | 'done' | 'all'
type Theme = 'light' | 'dark'
type NavigationMode = 'push' | 'replace'

type TimedCacheEntry<T> = {
  value: T
  expiresAt: number
}

const SAVED_EVENTS_STORAGE_PREFIX = 'rss-hub-reader:saved-events:v1:'
const RADAR_CACHE_TTL_MS = 60_000
const briefCache = new Map<string, TimedCacheEntry<NowBrief>>()
const wireCache = new Map<string, TimedCacheEntry<WireResponse>>()
const watchesCache = new Map<string, TimedCacheEntry<Watch[]>>()
const detailCache = new Map<string, TimedCacheEntry<EventDetail>>()

function readTimedCache<T>(cache: Map<string, TimedCacheEntry<T>>, key: string): T | null {
  const entry = cache.get(key)
  if (!entry) return null
  if (entry.expiresAt <= Date.now()) {
    cache.delete(key)
    return null
  }
  return entry.value
}

function writeTimedCache<T>(cache: Map<string, TimedCacheEntry<T>>, key: string, value: T) {
  cache.set(key, { value, expiresAt: Date.now() + RADAR_CACHE_TTL_MS })
}

function readRadarView(): RadarView {
  if (typeof window === 'undefined') return 'now'
  const value = new URLSearchParams(window.location.search).get('radar')
  return value === 'wire' || value === 'watches' ? value : 'now'
}

function readRadarEventId(): string | null {
  if (typeof window === 'undefined') return null
  return new URLSearchParams(window.location.search).get('event')
}

function writeRadarLocation(view: RadarView, eventId: string | null, mode: NavigationMode) {
  const url = new URL(window.location.href)
  if (view === 'now') url.searchParams.delete('radar')
  else url.searchParams.set('radar', view)
  if (eventId) url.searchParams.set('event', eventId)
  else url.searchParams.delete('event')
  window.history[mode === 'push' ? 'pushState' : 'replaceState'](
    { ...window.history.state, radarView: view, eventId },
    '',
    url
  )
}

function savedEventsStorageKey(userName: string): string {
  return `${SAVED_EVENTS_STORAGE_PREFIX}${userName}`
}

function loadSavedEvents(userName: string): EventBriefItem[] {
  if (typeof window === 'undefined') return []
  try {
    const stored = window.localStorage.getItem(savedEventsStorageKey(userName))
    if (!stored) return []
    const parsed = JSON.parse(stored) as unknown
    if (!Array.isArray(parsed)) return []
    return parsed.filter((item): item is EventBriefItem => (
      typeof item === 'object' && item !== null
      && typeof (item as EventBriefItem).event_id === 'string'
      && typeof (item as EventBriefItem).title === 'string'
    ))
  } catch {
    return []
  }
}

function truncateLabel(value: string, maxCharacters: number): string {
  const characters = Array.from(value.trim())
  return characters.length <= maxCharacters
    ? characters.join('')
    : `${characters.slice(0, Math.max(1, maxCharacters - 1)).join('')}…`
}

export function WorldRadar({
  userName,
  theme,
  onToggleTheme,
  onOpenLibrary,
  onOpenSettings
}: {
  userName: string
  theme: Theme
  onToggleTheme: () => void
  onOpenLibrary: (query?: string, articleId?: string) => void
  onOpenSettings: () => void
}) {
  const [view, setView] = useState<RadarView>(readRadarView)
  const [workflow, setWorkflow] = useState<BriefWorkflow>('pending')
  const [minutes, setMinutes] = useState(10)
  const [hours, setHours] = useState(24)
  const [brief, setBrief] = useState<NowBrief | null>(() => (
    readTimedCache(briefCache, `${userName}:10:24`)
  ))
  const [wire, setWire] = useState<WireResponse | null>(() => (
    readTimedCache(wireCache, `${userName}:24`)
  ))
  const [watches, setWatches] = useState<Watch[]>(() => (
    readTimedCache(watchesCache, userName) ?? []
  ))
  const [savedEvents, setSavedEvents] = useState<EventBriefItem[]>(() => loadSavedEvents(userName))
  const [selectedEventId, setSelectedEventId] = useState<string | null>(readRadarEventId)
  const [detail, setDetail] = useState<EventDetail | null>(() => {
    const eventId = readRadarEventId()
    return eventId ? readTimedCache(detailCache, `${userName}:${eventId}`) : null
  })
  const [question, setQuestion] = useState('')
  const [watchName, setWatchName] = useState('')
  const [watchQuery, setWatchQuery] = useState('')
  const [loading, setLoading] = useState(() => !readTimedCache(briefCache, `${userName}:10:24`))
  const [wireLoading, setWireLoading] = useState(false)
  const [watchesLoading, setWatchesLoading] = useState(() => !readTimedCache(watchesCache, userName))
  const [detailLoading, setDetailLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const loadWatches = useCallback(async (force = false) => {
    const cacheKey = userName
    const cached = force ? null : readTimedCache(watchesCache, cacheKey)
    if (cached) {
      setWatches(cached)
      setWatchesLoading(false)
      return
    }
    setWatchesLoading(true)
    try {
      const next = await api<Watch[]>('/api/v2/watches')
      writeTimedCache(watchesCache, cacheKey, next)
      setWatches(next)
    } catch (err) {
      setError(err instanceof Error ? err.message : '关注加载失败')
    } finally {
      setWatchesLoading(false)
    }
  }, [userName])

  const loadBrief = useCallback(async (force = false) => {
    const cacheKey = `${userName}:${minutes}:${hours}`
    const cached = force ? null : readTimedCache(briefCache, cacheKey)
    if (cached) {
      setBrief(cached)
      setLoading(false)
      return
    }
    setLoading(true)
    setError(null)
    try {
      const next = await api<NowBrief>(`/api/v2/brief/now?minutes=${minutes}&hours=${hours}`)
      writeTimedCache(briefCache, cacheKey, next)
      setBrief(next)
    } catch (err) {
      setError(err instanceof Error ? err.message : '简报加载失败')
    } finally {
      setLoading(false)
    }
  }, [hours, minutes, userName])

  const loadWire = useCallback(async (force = false) => {
    const cacheKey = `${userName}:${hours}`
    const cached = force ? null : readTimedCache(wireCache, cacheKey)
    if (cached) {
      setWire(cached)
      setWireLoading(false)
      return
    }
    setWireLoading(true)
    setError(null)
    try {
      const next = await api<WireResponse>(`/api/v2/wire/now?hours=${hours}&limit=40`)
      writeTimedCache(wireCache, cacheKey, next)
      setWire(next)
    } catch (err) {
      setError(err instanceof Error ? err.message : '快讯加载失败')
    } finally {
      setWireLoading(false)
    }
  }, [hours, userName])

  useEffect(() => {
    void loadBrief()
  }, [loadBrief])

  useEffect(() => {
    const timer = window.setTimeout(() => void loadWatches(), 250)
    return () => window.clearTimeout(timer)
  }, [loadWatches])

  useEffect(() => {
    if (view !== 'wire') return
    void loadWire()
    const timer = window.setInterval(() => void loadWire(true), 30_000)
    return () => window.clearInterval(timer)
  }, [loadWire, view])

  useEffect(() => {
    function onPopState() {
      setView(readRadarView())
      setSelectedEventId(readRadarEventId())
    }
    window.addEventListener('popstate', onPopState)
    return () => window.removeEventListener('popstate', onPopState)
  }, [])

  useEffect(() => {
    if (!selectedEventId) {
      setDetail(null)
      return
    }
    const cacheKey = `${userName}:${selectedEventId}`
    const cached = readTimedCache(detailCache, cacheKey)
    if (cached) {
      setDetail(cached)
      setDetailLoading(false)
      return
    }
    const controller = new AbortController()
    setDetail(null)
    setDetailLoading(true)
    api<EventDetail>(`/api/v2/events/${selectedEventId}`, { signal: controller.signal })
      .then((next) => {
        writeTimedCache(detailCache, cacheKey, next)
        setDetail(next)
      })
      .catch((err) => {
        if (err instanceof DOMException && err.name === 'AbortError') return
        setError(err instanceof Error ? err.message : '事件详情加载失败')
      })
      .finally(() => {
        if (!controller.signal.aborted) setDetailLoading(false)
      })
    return () => controller.abort()
  }, [selectedEventId, userName])

  const activeWatches = useMemo(() => watches.filter((watch) => watch.enabled), [watches])
  const briefItems = useMemo(() => brief?.items ?? [], [brief])
  const savedEventIds = useMemo(() => new Set(savedEvents.map((item) => item.event_id)), [savedEvents])
  const completedCount = useMemo(
    () => briefItems.filter((item) => item.feedback === 'known').length,
    [briefItems]
  )
  const dismissedCount = useMemo(
    () => briefItems.filter((item) => ['less', 'irrelevant', 'bad_cluster'].includes(item.feedback ?? '')).length,
    [briefItems]
  )
  const processedCount = useMemo(
    () => briefItems.filter((item) => (
      item.feedback === 'known'
      || ['less', 'irrelevant', 'bad_cluster'].includes(item.feedback ?? '')
      || savedEventIds.has(item.event_id)
    )).length,
    [briefItems, savedEventIds]
  )
  const completionPercent = briefItems.length ? Math.round(processedCount / briefItems.length * 100) : 0
  const visibleItems = useMemo(() => {
    if (workflow === 'saved') return savedEvents
    if (workflow === 'done') return briefItems.filter((item) => item.feedback === 'known')
    if (workflow === 'all') return briefItems
    return briefItems.filter((item) => (
      !['known', 'less', 'irrelevant', 'bad_cluster'].includes(item.feedback ?? '')
      && !savedEventIds.has(item.event_id)
    ))
  }, [briefItems, savedEventIds, savedEvents, workflow])

  useEffect(() => {
    setSelectedEventId((current) => (
      (current && readRadarEventId() === current) || visibleItems.some((item) => item.event_id === current)
        ? current
        : visibleItems[0]?.event_id ?? null
    ))
  }, [visibleItems])

  useEffect(() => {
    try {
      window.localStorage.setItem(savedEventsStorageKey(userName), JSON.stringify(savedEvents))
    } catch {
      // The brief remains usable when storage is unavailable; only "later" persistence is lost.
    }
  }, [savedEvents, userName])

  useEffect(() => {
    if (!notice) return
    const timer = window.setTimeout(() => setNotice(null), 4_000)
    return () => window.clearTimeout(timer)
  }, [notice])

  async function createWatch(event: FormEvent) {
    event.preventDefault()
    if (!watchName.trim() || !watchQuery.trim()) return
    setSaving(true)
    setError(null)
    try {
      const payload: WatchRequest = {
        name: watchName.trim(),
        query: watchQuery.trim(),
        min_impact: 40,
        notification_mode: 'brief',
        enabled: true
      }
      await api<Watch>('/api/v2/watches', {
        method: 'POST',
        body: JSON.stringify(payload)
      })
      setWatchName('')
      setWatchQuery('')
      setNotice('关注已创建')
      await Promise.all([loadWatches(true), loadBrief(true)])
    } catch (err) {
      setError(err instanceof Error ? err.message : '关注创建失败')
    } finally {
      setSaving(false)
    }
  }

  async function trackEvent(event: EventBriefItem) {
    const existing = watches.find((watch) => watch.query.trim().toLocaleLowerCase() === event.title.trim().toLocaleLowerCase())
    if (existing?.enabled) {
      setNotice('已在追踪')
      return
    }
    if (existing) {
      if (await toggleWatch(existing)) setNotice('已恢复追踪')
      return
    }

    setSaving(true)
    setError(null)
    try {
      const payload: WatchRequest = {
        name: `追踪 · ${truncateLabel(event.title, 55)} · ${event.event_id.slice(0, 6)}`,
        query: truncateLabel(event.title, 500),
        min_impact: 40,
        notification_mode: 'brief',
        enabled: true
      }
      await api<Watch>('/api/v2/watches', {
        method: 'POST',
        body: JSON.stringify(payload)
      })
      setNotice('已开始追踪')
      await Promise.all([loadWatches(true), loadBrief(true)])
    } catch (err) {
      setError(err instanceof Error ? err.message : '事件追踪失败')
    } finally {
      setSaving(false)
    }
  }

  function toggleSavedEvent(event: EventBriefItem) {
    setSavedEvents((current) => (
      current.some((item) => item.event_id === event.event_id)
        ? current.filter((item) => item.event_id !== event.event_id)
        : [event, ...current]
    ))
    setNotice(savedEventIds.has(event.event_id) ? '已移出稍后处理' : '已加入稍后处理')
  }

  async function toggleWatch(watch: Watch): Promise<boolean> {
    setSaving(true)
    setError(null)
    try {
      await api<Watch>(`/api/v2/watches/${watch.id}`, {
        method: 'PATCH',
        body: JSON.stringify({
          name: watch.name,
          query: watch.query,
          min_impact: watch.min_impact,
          notification_mode: watch.notification_mode,
          enabled: !watch.enabled
        } satisfies WatchRequest)
      })
      await Promise.all([loadWatches(true), loadBrief(true)])
      return true
    } catch (err) {
      setError(err instanceof Error ? err.message : '关注更新失败')
      return false
    } finally {
      setSaving(false)
    }
  }

  async function deleteWatch(watch: Watch) {
    if (!window.confirm(`删除关注「${watch.name}」？`)) return
    setSaving(true)
    setError(null)
    try {
      await api(`/api/v2/watches/${watch.id}`, { method: 'DELETE' })
      await Promise.all([loadWatches(true), loadBrief(true)])
    } catch (err) {
      setError(err instanceof Error ? err.message : '关注删除失败')
    } finally {
      setSaving(false)
    }
  }

  async function sendFeedback(kind: 'more' | 'less' | 'known' | 'irrelevant' | 'bad_cluster') {
    if (!selectedEventId) return
    setSaving(true)
    setError(null)
    try {
      await api('/api/v2/feedback', {
        method: 'POST',
        body: JSON.stringify({ event_id: selectedEventId, kind })
      })
      setBrief((current) => {
        if (!current) return current
        const next = {
          ...current,
          items: current.items.map((item) => item.event_id === selectedEventId ? { ...item, feedback: kind } : item)
        }
        writeTimedCache(briefCache, `${userName}:${minutes}:${hours}`, next)
        return next
      })
      setDetail((current) => {
        if (!current || current.event.event_id !== selectedEventId) return current
        const next = { ...current, event: { ...current.event, feedback: kind } }
        writeTimedCache(detailCache, `${userName}:${selectedEventId}`, next)
        return next
      })
      setSavedEvents((current) => current.map((item) => (
        item.event_id === selectedEventId ? { ...item, feedback: kind } : item
      )))
      if (kind === 'known') setNotice('已完成')
      else if (kind === 'more') setNotice('偏好已更新')
      else if (kind === 'bad_cluster') setNotice('已排除并记录聚类问题')
      else setNotice('已移出待处理')
    } catch (err) {
      setError(err instanceof Error ? err.message : '反馈保存失败')
    } finally {
      setSaving(false)
    }
  }

  async function regenerateDigest() {
    if (!selectedEventId) return
    setSaving(true)
    setError(null)
    try {
      const result = await api<{ queued: boolean }>(`/api/v2/events/${selectedEventId}/digest`, {
        method: 'POST'
      })
      detailCache.delete(`${userName}:${selectedEventId}`)
      setNotice(result.queued ? '已加入核验队列' : '正在核验')
      await loadWire(true)
    } catch (err) {
      setError(err instanceof Error ? err.message : '重新核验失败')
    } finally {
      setSaving(false)
    }
  }

  function submitQuestion(event: FormEvent) {
    event.preventDefault()
    if (!question.trim()) return
    onOpenLibrary(question.trim())
  }

  function navigateRadar(nextView: RadarView, eventId = selectedEventId) {
    if (nextView === view) return
    if (nextView === 'wire' && !wire) setWireLoading(true)
    if (nextView === 'watches' && watches.length === 0) setWatchesLoading(true)
    writeRadarLocation(nextView, eventId, 'push')
    setView(nextView)
  }

  function selectEvent(eventId: string) {
    writeRadarLocation(view, eventId, 'replace')
    setSelectedEventId(eventId)
  }

  function openWireEvent(eventId: string) {
    writeRadarLocation('now', eventId, 'push')
    setSelectedEventId(eventId)
    setView('now')
  }

  return (
    <div className="radar-shell">
      <a className="skip-link" href="#radar-main">跳到简报</a>
      <header className="radar-topbar">
        <div className="radar-topbar-spacer" aria-hidden="true" />
        <nav className="radar-nav" aria-label="主要导航">
          <button type="button" className={view === 'now' ? 'active' : ''} aria-label="现在" aria-current={view === 'now' ? 'page' : undefined} onClick={() => navigateRadar('now')}>
            <Compass size={16} /><span>现在</span>
          </button>
          <button type="button" className={view === 'wire' ? 'active' : ''} aria-label="快讯" aria-current={view === 'wire' ? 'page' : undefined} onClick={() => navigateRadar('wire')}>
            <Radio size={16} /><span>快讯</span>
            {wire && wire.stats.running > 0 && <b>{wire.stats.running}</b>}
          </button>
          <button type="button" className={view === 'watches' ? 'active' : ''} aria-label="关注" aria-current={view === 'watches' ? 'page' : undefined} onClick={() => navigateRadar('watches')}>
            <Target size={16} /><span>关注</span>
            {activeWatches.length > 0 && <b>{activeWatches.length}</b>}
          </button>
          <button type="button" aria-label="全部资料" onClick={() => onOpenLibrary()}>
            <Library size={16} /><span>全部资料</span>
          </button>
        </nav>
        <div className="radar-top-actions">
          <button type="button" className="theme-toggle" onClick={onToggleTheme} aria-label={`切换到${theme === 'dark' ? '浅色' : '深色'}主题`}>
            {theme === 'dark' ? '浅色' : '深色'}
          </button>
          <button type="button" className="theme-toggle" onClick={onOpenSettings} aria-label="打开管理设置">
            <Settings size={16} />
          </button>
        </div>
      </header>

      {(notice || error) && (
        <div className={error ? 'radar-status error' : 'radar-status'} role={error ? 'alert' : 'status'}>
          <span>{error ?? notice}</span>
          <button type="button" onClick={() => { setNotice(null); setError(null) }} aria-label="关闭提示">×</button>
        </div>
      )}

      {view === 'now' ? (
        <main id="radar-main" className="radar-main" tabIndex={-1}>
          <section className="radar-overview">
            <div>
              <p className="radar-context"><span aria-hidden="true" />{hours <= 24 ? `过去 ${hours} 小时` : `过去 ${Math.round(hours / 24)} 天`}</p>
              <h1>真正发生了什么</h1>
            </div>
            <div className="radar-controls">
              <div className="radar-segment" aria-label="阅读时间预算">
                {[3, 10, 20].map((value) => (
                  <button type="button" key={value} className={minutes === value ? 'active' : ''} aria-pressed={minutes === value} onClick={() => setMinutes(value)}>
                    {value} 分钟
                  </button>
                ))}
              </div>
              <div className="radar-segment" aria-label="信息时间范围">
                {[12, 24, 72].map((value) => (
                  <button type="button" key={value} className={hours === value ? 'active' : ''} aria-pressed={hours === value} onClick={() => setHours(value)}>
                    {value < 24 ? `${value}h` : `${value / 24}d`}
                  </button>
                ))}
              </div>
              <button type="button" className="radar-refresh" onClick={() => void loadBrief(true)} disabled={loading}>
                <RefreshCw size={16} className={loading ? 'spin' : ''} />刷新
              </button>
            </div>
          </section>

          <form className="radar-question" onSubmit={submitQuestion}>
            <Search size={18} />
            <input
              value={question}
              onChange={(event) => setQuestion(event.target.value)}
              placeholder="搜索证据或问题"
              aria-label="搜索证据或提问"
            />
            <button type="submit">搜索 <ArrowRight size={16} /></button>
          </form>

          {brief && (
            <div className="radar-metrics" aria-label="简报统计">
              <span><strong>{brief.stats.selected_events}</strong> 事件</span>
              <span><strong>{brief.stats.eligible_events}</strong> / {brief.stats.candidate_events} 通过</span>
              <span><strong>{brief.stats.independent_sources}</strong> 来源</span>
              <span><strong>{brief.stats.watch_matches}</strong> 关注</span>
            </div>
          )}

          {brief && (
            <section className="brief-workflow" aria-label="简报处理进度">
              <div className="brief-progress-copy">
                <span className="brief-progress-icon" aria-hidden="true"><ListChecks size={18} /></span>
                <div>
                  <strong>{processedCount === briefItems.length && briefItems.length > 0 ? '已处理完成' : `已处理 ${processedCount}/${briefItems.length}`}</strong>
                  <span>完成 {completedCount} · 排除 {dismissedCount} · 稍后 {savedEvents.length}</span>
                </div>
                <div className="brief-progress-track" role="progressbar" aria-label="简报处理进度" aria-valuemin={0} aria-valuemax={100} aria-valuenow={completionPercent}>
                  <span style={{ transform: `scaleX(${completionPercent / 100})` }} />
                </div>
              </div>
              <div className="brief-workflow-tabs" aria-label="简报状态筛选">
                {([
                  ['pending', '待处理', Math.max(0, briefItems.length - processedCount)],
                  ['saved', '稍后', savedEvents.length],
                  ['done', '已完成', completedCount],
                  ['all', '全部', briefItems.length]
                ] as const).map(([value, label, count]) => (
                  <button type="button" key={value} className={workflow === value ? 'active' : ''} aria-pressed={workflow === value} onClick={() => setWorkflow(value)}>
                    {label}<b>{count}</b>
                  </button>
                ))}
              </div>
            </section>
          )}

          <div className="radar-workspace">
            <section className="event-feed" aria-label="事件简报" aria-busy={loading}>
              {loading && !brief ? (
                <RadarSkeleton label="正在更新" />
              ) : visibleItems.length ? (
                visibleItems.map((item, index) => (
                  <EventCard
                    key={item.event_id}
                    item={item}
                    index={index + 1}
                    active={selectedEventId === item.event_id}
                    saved={savedEventIds.has(item.event_id)}
                    onSelect={() => selectEvent(item.event_id)}
                  />
                ))
              ) : (
                <BriefWorkflowEmpty workflow={workflow} hasBriefItems={briefItems.length > 0} />
              )}
            </section>

            <aside className="event-inspector" aria-label="事件详情">
              {detailLoading && !detail ? (
                <RadarSkeleton compact label="正在读取事件时间线" />
              ) : detail ? (
                <EventInspector
                  detail={detail}
                  saving={saving}
                  saved={savedEventIds.has(detail.event.event_id)}
                  tracked={watches.some((watch) => watch.enabled && watch.query.trim().toLocaleLowerCase() === detail.event.title.trim().toLocaleLowerCase())}
                  onOpenArticle={(articleId) => onOpenLibrary(undefined, articleId)}
                  onFeedback={sendFeedback}
                  onRegenerate={regenerateDigest}
                  onToggleSaved={() => toggleSavedEvent(detail.event)}
                  onTrack={() => trackEvent(detail.event)}
                />
              ) : (
                <div className="radar-empty"><Eye />选择事件查看详情</div>
              )}
            </aside>
          </div>
        </main>
      ) : view === 'wire' ? (
        <main id="radar-main" className="wire-main" tabIndex={-1}>
          <section className="wire-heading">
            <div>
              <p className="radar-context"><span aria-hidden="true" />持续核验</p>
              <h1>快讯</h1>
            </div>
            <div className="radar-controls">
              <div className="radar-segment" aria-label="快讯时间范围">
                {[12, 24, 72].map((value) => (
                  <button type="button" key={value} className={hours === value ? 'active' : ''} aria-pressed={hours === value} onClick={() => setHours(value)}>
                    {value < 24 ? `${value}h` : `${value / 24}d`}
                  </button>
                ))}
              </div>
              <button type="button" className="radar-refresh" onClick={() => void loadWire(true)} disabled={wireLoading}>
                <RefreshCw size={16} className={wireLoading ? 'spin' : ''} />刷新
              </button>
            </div>
          </section>
          {wire && (
            <div className="wire-agent-status" aria-label="事件 Agent 状态">
              <span><strong>{wire.stats.published}</strong> 已发布</span>
              <span><strong>{wire.stats.pending}</strong> 待处理</span>
              <span><strong>{wire.stats.running}</strong> 核验中</span>
              {wire.stats.skipped > 0 && <span><strong>{wire.stats.skipped}</strong> 未发布</span>}
              {wire.stats.failed > 0 && <span className="failed"><strong>{wire.stats.failed}</strong> 失败</span>}
            </div>
          )}
          <section className="wire-feed" aria-label="事件快讯" aria-busy={wireLoading}>
            {wireLoading && !wire ? (
              <RadarSkeleton label="正在更新" />
            ) : wire?.items.length ? wire.items.map((item) => (
              <button
                type="button"
                className={`wire-item status-${item.verification_status}`}
                key={item.event_id}
                onClick={() => openWireEvent(item.event_id)}
              >
                <div className="wire-time">{formatDate(item.published_at)}</div>
                <div className="wire-copy">
                  <p>{item.message}</p>
                  <div>
                    <span className="wire-verification"><ShieldCheck size={13} />{item.verification_label}</span>
                    <span>{item.cited_source_count}/{item.source_count} 源支撑</span>
                    <span>材料分 {item.verification_score}</span>
                  </div>
                  <small>{item.verification_reason}</small>
                </div>
                <ArrowRight size={17} />
              </button>
            )) : (
              <div className="radar-empty"><Radio />暂无快讯</div>
            )}
          </section>
        </main>
      ) : (
        <main id="radar-main" className="watch-main" tabIndex={-1}>
          <section className="watch-heading">
            <div>
              <h1>关注</h1>
            </div>
            <BellRing size={34} />
          </section>
          <form className="watch-create" onSubmit={createWatch}>
            <label>
              关注名称
              <input value={watchName} onChange={(event) => setWatchName(event.target.value)} placeholder="例如：AI Agent 生产化" />
            </label>
            <label>
              主题、实体或问题
              <input value={watchQuery} onChange={(event) => setWatchQuery(event.target.value)} placeholder="OpenAI Agent SDK 生产部署" />
            </label>
            <button type="submit" disabled={saving || !watchName.trim() || !watchQuery.trim()}>
              <Plus size={17} />创建关注
            </button>
          </form>
          <section className="watch-list" aria-label="关注列表" aria-busy={watchesLoading}>
            {watchesLoading && watches.length === 0 ? (
              <RadarSkeleton label="正在读取关注" />
            ) : watches.length ? watches.map((watch) => (
              <article key={watch.id} className={watch.enabled ? 'watch-card' : 'watch-card disabled'}>
                <div className="watch-icon"><Target size={20} /></div>
                <div>
                  <div className="watch-title-line">
                    <h2>{watch.name}</h2>
                    <span>{watch.notification_mode === 'urgent' ? '紧急通知' : watch.notification_mode === 'brief' ? '进入简报' : '仅追踪'}</span>
                  </div>
                  <p>{watch.query}</p>
                  <small>影响门槛 {watch.min_impact} · {watch.enabled ? '正在参与排序' : '已暂停'}</small>
                </div>
                <div className="watch-actions">
                  <button type="button" onClick={() => void toggleWatch(watch)} disabled={saving}>
                    {watch.enabled ? <XCircle size={17} /> : <CheckCircle2 size={17} />}
                    {watch.enabled ? '暂停' : '启用'}
                  </button>
                  <button type="button" className="danger" onClick={() => void deleteWatch(watch)} disabled={saving} aria-label={`删除 ${watch.name}`}>
                    <Trash2 size={17} />
                  </button>
                </div>
              </article>
            )) : (
              <div className="radar-empty"><Target />暂无关注</div>
            )}
          </section>
        </main>
      )}
    </div>
  )
}

function EventCard({
  item,
  index,
  active,
  saved,
  onSelect
}: {
  item: EventBriefItem
  index: number
  active: boolean
  saved: boolean
  onSelect: () => void
}) {
  const stateClass = item.feedback === 'known' ? ' completed' : saved ? ' saved' : ''
  return (
    <button type="button" className={`event-card${active ? ' active' : ''}${stateClass}`} onClick={onSelect} aria-pressed={active}>
      <span className="event-index">{String(index).padStart(2, '0')}</span>
      <div className="event-card-body">
        <div className="event-meta">
          <RevisionBadge type={item.revision_type} />
          {saved && <span className="workflow-state"><Bookmark size={13} />稍后</span>}
          {item.feedback === 'known' && <span className="workflow-state"><CheckCircle2 size={13} />已完成</span>}
          <span><Clock3 size={13} />{formatDate(item.published_at)}</span>
          <span><ShieldCheck size={13} />{item.confidence_label}</span>
        </div>
        <h2>{item.title}</h2>
        <p>{item.what_happened}</p>
        <div className="event-reasons">
          <span><TrendingUp size={14} />{item.why_now}</span>
          <span><Sparkles size={14} />{item.why_it_matters}</span>
        </div>
        <div className="event-card-footer">
          <div>
            <b>{item.total_score}</b><small>综合</small>
            <b>{item.confidence_score}</b><small>可信</small>
            <b>{item.independent_source_count}</b><small>来源</small>
          </div>
          {item.watch_names.map((watch) => <span className="watch-hit" key={watch}>命中 {watch}</span>)}
        </div>
      </div>
      <ArrowRight className="event-open" size={18} />
    </button>
  )
}

function EventInspector({
  detail,
  saving,
  saved,
  tracked,
  onOpenArticle,
  onFeedback,
  onRegenerate,
  onToggleSaved,
  onTrack
}: {
  detail: EventDetail
  saving: boolean
  saved: boolean
  tracked: boolean
  onOpenArticle: (articleId: string) => void
  onFeedback: (kind: 'more' | 'less' | 'known' | 'irrelevant' | 'bad_cluster') => Promise<void>
  onRegenerate: () => Promise<void>
  onToggleSaved: () => void
  onTrack: () => Promise<void>
}) {
  const { event } = detail
  return (
    <div className="event-inspector-content">
      <div className="inspector-heading">
        <RevisionBadge type={event.revision_type} />
        <span>{event.independent_source_count} 个订阅源去重 · 可信度 {event.confidence_score} · 来源质量 {event.source_quality_score}</span>
        <button type="button" disabled={saving} onClick={() => void onRegenerate()}>
          <RefreshCw size={13} />重新核验
        </button>
      </div>
      <h2>{event.title}</h2>
      <div className="event-primary-actions" aria-label="事件处理动作">
        <button type="button" className="complete" disabled={saving || event.feedback === 'known'} onClick={() => void onFeedback('known')}>
          <CheckCircle2 size={16} />{event.feedback === 'known' ? '已完成' : '标为完成'}
        </button>
        <button type="button" className={saved ? 'active' : ''} disabled={saving} aria-pressed={saved} onClick={onToggleSaved}>
          <Bookmark size={16} />{saved ? '移出稍后' : '稍后处理'}
        </button>
        <button type="button" className={tracked ? 'active' : ''} disabled={saving || tracked} aria-pressed={tracked} onClick={() => void onTrack()}>
          <Target size={16} />{tracked ? '持续追踪中' : '追踪事件'}
        </button>
      </div>
      <section className="inspector-summary">
        <h3>发生了什么</h3>
        <p>{event.what_happened}</p>
        <h3>为什么现在值得看</h3>
        <p>{event.why_now}</p>
        <h3>影响与边界</h3>
        <p>{event.why_it_matters}</p>
      </section>

      <section className="event-timeline">
        <div className="inspector-section-title"><Clock3 size={16} /><strong>事件 Ledger</strong></div>
        {detail.timeline.length ? detail.timeline.map((revision) => (
          <div className="timeline-row" key={revision.id}>
            <RevisionBadge type={revision.revision_type} compact />
            <div>
              <strong>{revision.title}</strong>
              <small>{formatDate(revision.occurred_at)} · 当时 {revision.source_count} 源</small>
            </div>
          </div>
        )) : <p className="inspector-empty">尚无持久修订记录。</p>}
      </section>

      <section className="event-evidence">
        <div className="inspector-section-title"><BookOpen size={16} /><strong>来源与证据</strong></div>
        {detail.evidence.map((evidence) => (
          <button type="button" key={evidence.article_id} onClick={() => onOpenArticle(evidence.article_id)}>
            <span>{evidence.source_title}<b>质量 {evidence.quality_score}</b></span>
            <strong>{evidence.title}</strong>
            <p>{evidence.excerpt}</p>
          </button>
        ))}
      </section>

      <section className="event-feedback">
        <strong>调整后续雷达</strong>
        <div>
          <button type="button" disabled={saving} onClick={() => void onFeedback('more')}>多看此类</button>
          <button type="button" disabled={saving} onClick={() => void onFeedback('less')}>少看此类</button>
          <button type="button" disabled={saving} onClick={() => void onFeedback('irrelevant')}>与我无关</button>
          <button type="button" disabled={saving} onClick={() => void onFeedback('known')}>我已知道</button>
          <button type="button" disabled={saving} onClick={() => void onFeedback('bad_cluster')}>聚类有误</button>
        </div>
      </section>
    </div>
  )
}

function BriefWorkflowEmpty({ workflow, hasBriefItems }: { workflow: BriefWorkflow; hasBriefItems: boolean }) {
  if (!hasBriefItems) {
    return <div className="radar-empty"><FileSearch />当前范围暂无事件</div>
  }
  if (workflow === 'pending') {
    return <div className="radar-empty complete"><CheckCircle2 />已处理完成</div>
  }
  if (workflow === 'saved') {
    return <div className="radar-empty"><Bookmark />暂无稍后项目</div>
  }
  if (workflow === 'done') {
    return <div className="radar-empty"><ListChecks />暂无已完成事件</div>
  }
  return <div className="radar-empty"><FileSearch />暂无事件</div>
}

function RadarSkeleton({ label, compact = false }: { label: string; compact?: boolean }) {
  return (
    <div className={compact ? 'radar-skeleton compact' : 'radar-skeleton'} role="status" aria-live="polite">
      <span className="skeleton-label">{label}…</span>
      {Array.from({ length: compact ? 3 : 4 }, (_, index) => (
        <span className="skeleton-line" key={index} aria-hidden="true" />
      ))}
    </div>
  )
}

function RevisionBadge({ type, compact = false }: { type: EventBriefItem['revision_type']; compact?: boolean }) {
  const labels: Record<EventBriefItem['revision_type'], string> = {
    ADD: '新增',
    UPDATE: '更新',
    CORRECT: '纠正',
    CONTRADICT: '分歧',
    CONFIRM: '确认'
  }
  return <span className={`revision-badge type-${type.toLowerCase()}${compact ? ' compact' : ''}`}>{labels[type]}</span>
}
