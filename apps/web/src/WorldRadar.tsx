import {
  ArrowLeft,
  ArrowRight,
  Bookmark,
  BookOpen,
  CheckCircle2,
  ChartNoAxesCombined,
  ChevronDown,
  Clock3,
  Compass,
  Cpu,
  Eye,
  FileSearch,
  Globe2,
  Inbox,
  Layers3,
  Library,
  ListChecks,
  Moon,
  Plus,
  Radio,
  RefreshCw,
  Search,
  Settings,
  ShieldCheck,
  Sun,
  Target,
  Trash2,
  TrendingUp,
  X,
  XCircle,
  type LucideIcon
} from 'lucide-react'
import { FormEvent, lazy, memo, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { OsintPanel } from './OsintPanel'
import { ReportFilterControls } from './ReportFilterControls'
import { EMPTY_REPORT_FILTERS, filterReportItems, hasReportFilters, type ReportFilters } from './reportFilters'
import { SituationBoard } from './SituationBoard'
import { EvidenceComparison, SourceCoverage } from './NewsCoverage'
import { coverageLabel, matchesCoverage, type CoverageFilter } from './coverageModel'
import {
  api,
  ApiError,
  EventBriefItem,
  EventDetail,
  formatDate,
  NowBrief,
  takePrefetched,
  WireResponse,
  Watch,
  watchDisplayName,
  WatchRequest
} from './api'

const OsintCasebook = lazy(() => import('./OsintCasebook').then((module) => ({ default: module.OsintCasebook })))

type RadarView = 'now' | 'wire' | 'watches' | 'investigations'
type BriefWorkflow = 'pending' | 'saved' | 'done' | 'all'
type ReportLayout = 'list' | 'board'
type Theme = 'light' | 'dark'
type NavigationMode = 'push' | 'replace'
type ReportSectionKey = EventBriefItem['report_section']

type TimedCacheEntry<T> = {
  value: T
  expiresAt: number
}

const SAVED_EVENTS_STORAGE_PREFIX = 'infofetcher:saved-events:v1:'
const LEGACY_SAVED_EVENTS_STORAGE_PREFIX = 'rss-hub-reader:saved-events:v1:'
const RADAR_CACHE_TTL_MS = 60_000
const briefCache = new Map<string, TimedCacheEntry<NowBrief>>()
const wireCache = new Map<string, TimedCacheEntry<WireResponse>>()
const watchesCache = new Map<string, TimedCacheEntry<Watch[]>>()
const detailCache = new Map<string, TimedCacheEntry<EventDetail>>()
const REPORT_SECTIONS: Array<{ key: ReportSectionKey; label: string; icon: LucideIcon }> = [
  { key: 'world', label: '全球与公共事务', icon: Globe2 },
  { key: 'technology', label: 'AI 与科技', icon: Cpu },
  { key: 'security', label: '安全与风险', icon: ShieldCheck },
  { key: 'business', label: '商业与经济', icon: ChartNoAxesCombined },
  { key: 'general', label: '其他重要变化', icon: Layers3 }
]

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
  return value === 'wire' || value === 'watches' || value === 'investigations' ? value : 'now'
}

function readRadarEventId(): string | null {
  if (typeof window === 'undefined') return null
  return new URLSearchParams(window.location.search).get('event')
}

function readReportLayout(): ReportLayout {
  return typeof window !== 'undefined' && new URLSearchParams(window.location.search).get('report_layout') === 'board' ? 'board' : 'list'
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
      ?? window.localStorage.getItem(`${LEGACY_SAVED_EVENTS_STORAGE_PREFIX}${userName}`)
    if (!stored) return []
    const parsed = JSON.parse(stored) as unknown
    if (!Array.isArray(parsed)) return []
    return parsed.filter((item): item is EventBriefItem => (
      typeof item === 'object' && item !== null
      && typeof (item as EventBriefItem).event_id === 'string'
      && typeof (item as EventBriefItem).title === 'string'
    )).map((item) => ({
      ...item,
      report_section: REPORT_SECTIONS.some((section) => section.key === item.report_section)
        ? item.report_section
        : 'general'
    }))
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
  const [reportLayout, setReportLayout] = useState<ReportLayout>(readReportLayout)
  const [topic, setTopic] = useState<ReportSectionKey | 'all'>('all')
  const [coverage, setCoverage] = useState<CoverageFilter>('all')
  const [reportFilters, setReportFilters] = useState<ReportFilters>(EMPTY_REPORT_FILTERS)
  const [reportHours, setReportHours] = useState(12)
  const [wireHours, setWireHours] = useState(24)
  const [brief, setBrief] = useState<NowBrief | null>(() => (
    readTimedCache(briefCache, `${userName}:report:12`)
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
  const [loading, setLoading] = useState(() => !readTimedCache(briefCache, `${userName}:report:12`))
  const [wireLoading, setWireLoading] = useState(false)
  const [watchesLoading, setWatchesLoading] = useState(() => !readTimedCache(watchesCache, userName))
  const [detailLoading, setDetailLoading] = useState(false)
  const [detailError, setDetailError] = useState<string | null>(null)
  const [detailRetry, setDetailRetry] = useState(0)
  const [saving, setSaving] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const [briefError, setBriefError] = useState<string | null>(null)
  const [wireError, setWireError] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [overviewOpen, setOverviewOpen] = useState(false)
  const overviewRef = useRef<HTMLDivElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const briefRequestRef = useRef<AbortController | null>(null)
  const wireRequestRef = useRef<AbortController | null>(null)
  const inspectorRef = useRef<HTMLElement>(null)
  const eventFeedRef = useRef<HTMLElement>(null)
  const selectedCardRef = useRef<HTMLButtonElement | null>(null)
  const selectedArticleId = brief?.items.find((item) => item.event_id === selectedEventId)?.representative_article_id
    ?? savedEvents.find((item) => item.event_id === selectedEventId)?.representative_article_id
    ?? wire?.items.find((item) => item.event_id === selectedEventId)?.citations.find((citation) => citation.article_id)?.article_id

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
    briefRequestRef.current?.abort()
    const controller = new AbortController()
    briefRequestRef.current = controller
    setBriefError(null)
    const cacheKey = `${userName}:report:${reportHours}`
    const cached = force ? null : readTimedCache(briefCache, cacheKey)
    if (cached) {
      setBrief(cached)
      setLoading(false)
      return
    }
    setBrief((current) => current?.window_hours === reportHours ? current : null)
    setLoading(true)
    setError(null)
    const path = `/api/v2/brief/now?hours=${reportHours}`
    try {
      const next = (force ? null : await takePrefetched<NowBrief>(path))
        ?? await api<NowBrief>(path, { signal: controller.signal })
      if (controller.signal.aborted) return
      writeTimedCache(briefCache, cacheKey, next)
      setBrief(next)
    } catch (err) {
      if (controller.signal.aborted) return
      setBriefError(err instanceof Error ? err.message : '简报加载失败')
    } finally {
      if (!controller.signal.aborted) setLoading(false)
    }
  }, [reportHours, userName])

  const loadWire = useCallback(async (force = false) => {
    wireRequestRef.current?.abort()
    const controller = new AbortController()
    wireRequestRef.current = controller
    setWireError(null)
    const cacheKey = `${userName}:${wireHours}`
    const cached = force ? null : readTimedCache(wireCache, cacheKey)
    if (cached) {
      setWire(cached)
      setWireLoading(false)
      wireRequestRef.current = null
      return
    }
    setWire((current) => current?.window_hours === wireHours ? current : null)
    setWireLoading(true)
    setError(null)
    try {
      const next = await api<WireResponse>(`/api/v2/wire/now?hours=${wireHours}&limit=40`, { signal: controller.signal })
      if (controller.signal.aborted) return
      writeTimedCache(wireCache, cacheKey, next)
      setWire(next)
    } catch (err) {
      if (controller.signal.aborted) return
      setWireError(err instanceof Error ? err.message : '快讯加载失败')
    } finally {
      if (wireRequestRef.current === controller) wireRequestRef.current = null
      if (!controller.signal.aborted) setWireLoading(false)
    }
  }, [userName, wireHours])

  useEffect(() => {
    void loadBrief()
    return () => briefRequestRef.current?.abort()
  }, [loadBrief])

  useEffect(() => {
    const timer = window.setTimeout(() => void loadWatches(), 250)
    return () => window.clearTimeout(timer)
  }, [loadWatches])

  useEffect(() => {
    if (view !== 'wire') return
    const refresh = (force = false) => {
      if (document.visibilityState !== 'visible' || wireRequestRef.current) return
      void loadWire(force)
    }
    const onVisibilityChange = () => refresh()
    refresh()
    const timer = window.setInterval(() => refresh(true), 30_000)
    document.addEventListener('visibilitychange', onVisibilityChange)
    return () => {
      window.clearInterval(timer)
      document.removeEventListener('visibilitychange', onVisibilityChange)
      wireRequestRef.current?.abort()
      wireRequestRef.current = null
    }
  }, [loadWire, view])

  useEffect(() => {
    function onPopState() {
      setView(readRadarView())
      setSelectedEventId(readRadarEventId())
      setReportLayout(readReportLayout())
    }
    window.addEventListener('popstate', onPopState)
    return () => window.removeEventListener('popstate', onPopState)
  }, [])

  useEffect(() => {
    if (!selectedEventId) {
      setDetail(null)
      setDetailError(null)
      setDetailLoading(false)
      return
    }
    const cacheKey = `${userName}:${selectedEventId}`
    const cached = readTimedCache(detailCache, cacheKey)
    if (cached) {
      setDetail(cached)
      setDetailError(null)
      setDetailLoading(false)
      return
    }
    const controller = new AbortController()
    setDetail(null)
    setDetailError(null)
    setDetailLoading(true)
    const query = selectedArticleId ? `?representative_article_id=${encodeURIComponent(selectedArticleId)}` : ''
    api<EventDetail>(`/api/v2/events/${selectedEventId}${query}`, { signal: controller.signal })
      .then((next) => {
        if (controller.signal.aborted) return
        const currentId = next.event.event_id
        writeTimedCache(detailCache, `${userName}:${currentId}`, next)
        setDetail(next)
        if (currentId !== selectedEventId) {
          briefCache.delete(`${userName}:report:${reportHours}`)
          wireCache.delete(`${userName}:${wireHours}`)
          setSavedEvents((current) => current.map((item) => item.event_id === selectedEventId ? next.event : item))
          writeRadarLocation(readRadarView(), currentId, 'replace')
          setSelectedEventId(currentId)
        }
      })
      .catch((err) => {
        if (controller.signal.aborted) return
        setDetailError(err instanceof ApiError && err.status === 404
          ? '事件已合并或过期，请刷新报告。'
          : err instanceof Error ? err.message : '事件详情加载失败')
      })
      .finally(() => {
        if (!controller.signal.aborted) setDetailLoading(false)
      })
    return () => controller.abort()
  }, [detailRetry, reportHours, selectedArticleId, selectedEventId, userName, wireHours])

  useEffect(() => {
    function focusSearch(event: KeyboardEvent) {
      if (view !== 'now' || event.isComposing || event.altKey) return
      const editing = event.target instanceof HTMLElement
        && (event.target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(event.target.tagName))
      if (((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k')
        || (!editing && !event.metaKey && !event.ctrlKey && event.key === '/')) {
        event.preventDefault()
        searchRef.current?.focus()
      }
    }
    window.addEventListener('keydown', focusSearch)
    return () => window.removeEventListener('keydown', focusSearch)
  }, [view])

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
  const workflowItems = useMemo(() => {
    if (workflow === 'saved') return savedEvents
    if (workflow === 'done') return briefItems.filter((item) => item.feedback === 'known')
    if (workflow === 'all') return briefItems
    return briefItems.filter((item) => (
      !['known', 'less', 'irrelevant', 'bad_cluster'].includes(item.feedback ?? '')
      && !savedEventIds.has(item.event_id)
    ))
  }, [briefItems, savedEventIds, savedEvents, workflow])
  const topicItems = useMemo(() => workflowItems.filter((item) => topic === 'all' || item.report_section === topic), [workflowItems, topic])
  const matchingItems = useMemo(() => filterReportItems(topicItems, reportFilters), [topicItems, reportFilters])
  const visibleItems = useMemo(() => matchingItems.filter((item) => matchesCoverage(item, coverage)), [matchingItems, coverage])
  const visibleReportSections = useMemo(() => {
    let index = 0
    return REPORT_SECTIONS.map((section) => ({
      ...section,
      items: visibleItems
        .filter((item) => item.report_section === section.key)
        .map((item) => ({ item, index: ++index }))
    })).filter((section) => section.items.length > 0)
  }, [visibleItems])

  useEffect(() => {
    setSelectedEventId((current) => (
      (current && readRadarEventId() === current) || visibleItems.some((item) => item.event_id === current)
        ? current
        : visibleReportSections[0]?.items[0]?.item.event_id ?? null
    ))
  }, [visibleItems, visibleReportSections])

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

  useEffect(() => {
    if (!overviewOpen) return
    const dismissOutside = (event: Event) => {
      const overview = overviewRef.current
      const target = event.target as Node
      if (!overview || overview.contains(target)) return
      // Clicking non-focusable panel text can focus its containing <main>.
      if (event.type === 'focusin' && target.contains(overview)) return
      setOverviewOpen(false)
    }
    document.addEventListener('pointerdown', dismissOutside, { passive: true })
    document.addEventListener('focusin', dismissOutside)
    return () => {
      document.removeEventListener('pointerdown', dismissOutside)
      document.removeEventListener('focusin', dismissOutside)
    }
  }, [overviewOpen])

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
    if (!window.confirm(`删除关注「${watchDisplayName(watch.name)}」？`)) return
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
        writeTimedCache(briefCache, `${userName}:report:${reportHours}`, next)
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

  const selectEvent = useCallback((eventId: string, card: HTMLButtonElement) => {
    writeRadarLocation(view, eventId, 'replace')
    setSelectedEventId(eventId)
    selectedCardRef.current = card
    if (window.matchMedia('(max-width: 1050px)').matches) {
      inspectorRef.current?.focus({ preventScroll: true })
      inspectorRef.current?.scrollIntoView({ block: 'start' })
    }
  }, [view])

  function returnToEvents() {
    const target = selectedCardRef.current?.isConnected ? selectedCardRef.current : eventFeedRef.current
    target?.focus({ preventScroll: true })
    target?.scrollIntoView({ block: 'center' })
  }

  function changeReportFilters(next: ReportFilters) {
    setReportFilters(next)
    writeRadarLocation(view, null, 'replace')
    setSelectedEventId(null)
  }

  function filterReport(nextTopic: ReportSectionKey | 'all', nextCoverage: CoverageFilter) {
    if (nextTopic === topic && nextCoverage === coverage) return
    setTopic(nextTopic)
    setCoverage(nextCoverage)
    // An explicit event link must not keep a filtered-out event in the inspector.
    writeRadarLocation(view, null, 'replace')
    setSelectedEventId(null)
  }

  function openWireEvent(eventId: string) {
    writeRadarLocation('now', eventId, 'push')
    setSelectedEventId(eventId)
    setView('now')
    changeReportLayout('list', eventId, 'replace')
  }

  function changeReportLayout(next: ReportLayout, eventId = selectedEventId, mode: NavigationMode = 'push') {
    const url = new URL(window.location.href)
    if (next === 'board') url.searchParams.set('report_layout', 'board')
    else url.searchParams.delete('report_layout')
    if (eventId) url.searchParams.set('event', eventId)
    else url.searchParams.delete('event')
    window.history[mode === 'push' ? 'pushState' : 'replaceState']({ ...window.history.state }, '', url)
    setReportLayout(next)
  }

  function openSituationEvent(eventId: string) {
    setWorkflow('all')
    setTopic('all')
    setCoverage('all')
    setSelectedEventId(eventId)
    selectedCardRef.current = null
    changeReportLayout('list', eventId)
    window.requestAnimationFrame(() => {
      inspectorRef.current?.focus({ preventScroll: true })
      inspectorRef.current?.scrollIntoView({ block: 'start' })
    })
  }

  return (
    <div className="radar-shell">
      <a className="skip-link" href="#radar-main">跳到简报</a>
      <header className={`radar-topbar${view === 'now' ? ' radar-topbar-now' : ''}`}>
        <div className="radar-topbar-spacer" aria-hidden="true" />
        <nav className="radar-nav" aria-label="主要导航">
          <button type="button" className={view === 'now' ? 'active' : ''} aria-label="报告" aria-current={view === 'now' ? 'page' : undefined} onClick={() => navigateRadar('now')}>
            <Compass size={16} /><span>报告</span>
          </button>
          <button type="button" className={view === 'wire' ? 'active' : ''} aria-label="快讯" aria-current={view === 'wire' ? 'page' : undefined} onClick={() => navigateRadar('wire')}>
            <Radio size={16} /><span>快讯</span>
            {wire && wire.stats.running > 0 && <b>{wire.stats.running}</b>}
          </button>
          <button type="button" className={view === 'watches' ? 'active' : ''} aria-label="关注" aria-current={view === 'watches' ? 'page' : undefined} onClick={() => navigateRadar('watches')}>
            <Target size={16} /><span>关注</span>
            {activeWatches.length > 0 && <b>{activeWatches.length}</b>}
          </button>
          <button type="button" className={view === 'investigations' ? 'active' : ''} aria-label="调查" aria-current={view === 'investigations' ? 'page' : undefined} onClick={() => navigateRadar('investigations')}>
            <FileSearch size={16} /><span>调查</span>
          </button>
          <button type="button" aria-label="全部资料" onClick={() => onOpenLibrary()}>
            <Library size={16} /><span>全部资料</span>
          </button>
        </nav>
        <div className="radar-top-actions">
          {view === 'now' && (
            <>
              <form className="radar-top-search" role="search" onSubmit={submitQuestion}>
                <Search size={16} />
                <input
                  ref={searchRef}
                  type="search"
                  enterKeyHint="search"
                  autoComplete="off"
                  aria-keyshortcuts="Control+k Meta+k /"
                  onKeyDown={(event) => { if (event.key === 'Escape' && !event.nativeEvent.isComposing) setQuestion('') }}
                  value={question}
                  onChange={(event) => setQuestion(event.target.value)}
                  placeholder="搜索证据或问题"
                  aria-label="搜索证据或提问"
                />
                {question ? (
                  <button type="button" aria-label="清除搜索" onClick={() => { setQuestion(''); searchRef.current?.focus() }}><X size={15} /></button>
                ) : <kbd className="search-shortcut" aria-hidden="true">/</kbd>}
                <button type="submit" aria-label="搜索" disabled={!question.trim()}><ArrowRight size={15} /></button>
              </form>
            </>
          )}
          <button type="button" className="theme-toggle radar-theme-action" onClick={onToggleTheme} title={`切换到${theme === 'dark' ? '浅色' : '深色'}主题`} aria-label={`切换到${theme === 'dark' ? '浅色' : '深色'}主题`}>
            {theme === 'dark' ? <Sun size={19} aria-hidden="true" /> : <Moon size={19} aria-hidden="true" />}
          </button>
          <button type="button" className="theme-toggle radar-settings-action" onClick={onOpenSettings} title="设置" aria-label="打开管理设置">
            <Settings size={19} aria-hidden="true" />
          </button>
        </div>
      </header>

      {(notice || error) && (
        <div className={error ? 'radar-status error' : 'radar-status'} role={error ? 'alert' : 'status'}>
          <span>{error ?? notice}</span>
          <button type="button" onClick={() => { setNotice(null); setError(null) }} aria-label="关闭提示">×</button>
        </div>
      )}

      {view === 'investigations' ? (
        <Suspense fallback={<main id="radar-main" className="osint-casebook" tabIndex={-1}><RadarSkeleton label="正在读取档案" /></main>}>
          <OsintCasebook />
        </Suspense>
      ) : view === 'now' ? (
        <main id="radar-main" className="radar-main" tabIndex={-1}>
          <section className="report-heading" aria-labelledby="report-title">
            <h1 id="report-title" className="visually-hidden">近 {reportHours} 小时</h1>
            <div className="report-view-controls">
              <div className="report-layout-toggle" aria-label="报告布局">
                <button type="button" aria-pressed={reportLayout === 'list'} onClick={() => changeReportLayout('list')}>列表</button>
                <button type="button" aria-pressed={reportLayout === 'board'} onClick={() => changeReportLayout('board')}>态势</button>
              </div>
              <label className="report-period">
                <Clock3 size={17} aria-hidden="true" />
                <select aria-label="报告覆盖范围" value={reportHours} onChange={(event) => setReportHours(Number(event.target.value))}>
                  <option value={12}>近 12 小时</option>
                  <option value={24}>近 24 小时</option>
                </select>
                <ChevronDown size={14} aria-hidden="true" />
              </label>
            </div>
            <div className="report-heading-actions">
              {brief && (
                <time dateTime={brief.generated_at} title={`${formatDate(brief.generated_at)} 生成`} aria-label={`${formatDate(brief.generated_at)} 生成`}>
                  {new Date(brief.generated_at).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}
                </time>
              )}
              <button type="button" className="report-icon-button" onClick={() => void loadBrief(true)} disabled={loading} title="刷新报告" aria-label="刷新全面报告">
                <RefreshCw size={18} className={loading ? 'spin' : ''} aria-hidden="true" />
              </button>
              {brief && (
                <div
                  ref={overviewRef}
                  className="report-overview"
                  onKeyDown={(event) => {
                    if (event.key !== 'Escape') return
                    setOverviewOpen(false)
                    event.currentTarget.querySelector('button')?.focus()
                  }}
                >
                  <button type="button" className="report-icon-button" title="报告概览" aria-label="报告概览" aria-expanded={overviewOpen} aria-controls="report-overview-panel" onClick={() => setOverviewOpen((open) => !open)}>
                    <ChartNoAxesCombined size={19} aria-hidden="true" />
                  </button>
                  {overviewOpen && <div id="report-overview-panel" className="report-overview-panel">
                    <h2>报告概览</h2>
                    <dl className="report-statistics">
                      <div><dt>事件</dt><dd>{brief.stats.selected_events}</dd></div>
                      <div><dt>通过筛选</dt><dd>{brief.stats.eligible_events} / {brief.stats.candidate_events}</dd></div>
                      <div><dt>独立来源</dt><dd>{brief.stats.independent_sources}</dd></div>
                      <div><dt>命中关注</dt><dd>{brief.stats.watch_matches}</dd></div>
                    </dl>
                    <div className="report-progress-caption">
                      <strong>已处理 {processedCount}/{briefItems.length}</strong>
                      <span>完成 {completedCount} · 排除 {dismissedCount}</span>
                    </div>
                    <div className="brief-progress-track" role="progressbar" aria-label="简报处理进度" aria-valuemin={0} aria-valuemax={100} aria-valuenow={completionPercent}>
                      <span style={{ transform: `scaleX(${completionPercent / 100})` }} />
                    </div>
                    {visibleReportSections.length > 1 && (
                      <nav className="report-directory" aria-label="报告主题目录">
                        {visibleReportSections.map((section) => (
                          <a key={section.key} href={`#report-section-${section.key}`} onClick={(event) => {
                            setOverviewOpen(false)
                            document.getElementById(`report-section-${section.key}`)?.focus({ preventScroll: true })
                          }}>
                            <section.icon size={16} aria-hidden="true" />{section.label}<span>{section.items.length}</span>
                          </a>
                        ))}
                      </nav>
                    )}
                    <time className="report-generated-at" dateTime={brief.generated_at}>{formatDate(brief.generated_at)} 生成</time>
                  </div>}
                </div>
              )}
            </div>
          </section>

          {briefError && <LoadError message={brief ? `${briefError}；当前显示上次成功生成的报告。` : briefError} onRetry={() => void loadBrief(true)} busy={loading} />}

          <div hidden={reportLayout !== 'board'} aria-busy={loading}>
            {brief ? <SituationBoard key={userName} brief={brief} userName={userName} filters={reportFilters} onFiltersChange={changeReportFilters} onOpenEvent={openSituationEvent} />
              : loading ? <RadarSkeleton label="正在读取态势" /> : null}
          </div>

          {brief && reportLayout === 'list' && (
            <nav className="report-topics" aria-label="报告主题筛选">
              <button type="button" aria-pressed={topic === 'all'} onClick={() => filterReport('all', coverage)}>全部主题<span>{workflowItems.length}</span></button>
              {REPORT_SECTIONS.filter((section) => section.key === topic || workflowItems.some((item) => item.report_section === section.key)).map((section) => (
                <button type="button" key={section.key} aria-pressed={topic === section.key} onClick={() => filterReport(section.key, coverage)}>
                  {section.label}<span>{workflowItems.filter((item) => item.report_section === section.key).length}</span>
                </button>
              ))}
            </nav>
          )}

          {brief && reportLayout === 'list' && (
            <section className="brief-workflow" aria-label="报告筛选">
              <div className="brief-workflow-tabs" aria-label="报告状态筛选">
                {([
                  ['pending', '待处理', Math.max(0, briefItems.length - processedCount), Inbox],
                  ['saved', '稍后', savedEvents.length, Bookmark],
                  ['done', '完成', completedCount, CheckCircle2],
                  ['all', '全部', briefItems.length, Layers3]
                ] as const).map(([value, label, count, Icon]) => (
                  <button type="button" key={value} className={workflow === value ? 'active' : ''} aria-label={`${label} ${count} 项`} aria-pressed={workflow === value} onClick={() => setWorkflow(value)}>
                    <Icon size={16} aria-hidden="true" /><span>{label}</span>{workflow === value && <b aria-hidden="true">{count}</b>}
                  </button>
                ))}
              </div>
            </section>
          )}

          <div className="radar-workspace" hidden={reportLayout !== 'list'}>
            <section ref={eventFeedRef} className="event-feed" aria-label="全面事件报告" aria-busy={loading} tabIndex={-1}>
              {brief && <ReportFilterControls items={workflowItems} value={reportFilters} onChange={changeReportFilters} />}
              {brief && <div className="coverage-filters" aria-label="来源覆盖筛选">
                <div>{([
                  ['all', '全部覆盖'], ['multiple', '多源报道'], ['single', '单一来源']
                ] as const).map(([value, label]) => <button type="button" key={value} aria-pressed={coverage === value} onClick={() => filterReport(topic, value)}>
                  {label}<span>{matchingItems.filter((item) => matchesCoverage(item, value)).length}</span>
                </button>)}</div>
                <span className="coverage-result-count" role="status">{visibleItems.length} 项事件</span>
              </div>}
              {loading && !brief ? (
                <RadarSkeleton label="正在生成全面报告" />
              ) : visibleItems.length ? (
                <div className="report-sections">
                  {visibleReportSections.map((section) => (
                    <section className="report-section" key={section.key} aria-labelledby={`report-section-${section.key}`}>
                      <header>
                        <div>
                          <section.icon size={18} aria-hidden="true" />
                          <h2 id={`report-section-${section.key}`} tabIndex={-1}>{section.label}</h2>
                        </div>
                        <span>{section.items.length} 项</span>
                      </header>
                      <div>
                        {section.items.map(({ item, index }) => (
                          <EventCard
                            key={item.event_id}
                            item={item}
                            index={index}
                            active={selectedEventId === item.event_id}
                            saved={savedEventIds.has(item.event_id)}
                            onSelect={selectEvent}
                          />
                        ))}
                      </div>
                    </section>
                  ))}
                </div>
              ) : (
                !briefError && (topic !== 'all' || coverage !== 'all' || hasReportFilters(reportFilters) ? <div className="radar-empty">
                  <FileSearch aria-hidden="true" /><p>没有符合筛选的事件</p>
                  <button type="button" className="radar-refresh" onClick={() => { filterReport('all', 'all'); changeReportFilters(EMPTY_REPORT_FILTERS) }}>清除筛选</button>
                </div> : <BriefWorkflowEmpty workflow={workflow} hasBriefItems={briefItems.length > 0} />)
              )}
            </section>

            <aside ref={inspectorRef} className="event-inspector" aria-label="事件详情" aria-busy={detailLoading} tabIndex={-1}>
              <button type="button" className="inspector-back" onClick={returnToEvents}>
                <ArrowLeft size={16} />返回事件列表
              </button>
              {detailLoading && !detail ? (
                <RadarSkeleton compact label="正在读取事件时间线" />
              ) : detail ? (
                <EventInspector
                  key={detail.event.event_id}
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
              ) : detailError ? (
                <div className="radar-empty" role="alert">
                  <Eye /><p>{detailError}</p>
                  <button type="button" className="radar-refresh" onClick={() => { setDetailRetry((value) => value + 1); void loadBrief(true) }}>
                    <RefreshCw size={16} />刷新报告
                  </button>
                  {savedEventIds.has(selectedEventId ?? '') && <button type="button" className="radar-refresh" onClick={() => {
                    setSavedEvents((current) => current.filter((item) => item.event_id !== selectedEventId))
                    writeRadarLocation(readRadarView(), null, 'replace')
                    setSelectedEventId(null)
                  }}>移出稍后</button>}
                </div>
              ) : (
                <div className="radar-empty"><Eye />选择事件查看详情</div>
              )}
            </aside>
          </div>
        </main>
      ) : view === 'wire' ? (
        <main id="radar-main" className="wire-main" tabIndex={-1}>
          <section className="wire-heading" aria-labelledby="wire-title">
            <h1 id="wire-title" className="visually-hidden">快讯</h1>
            <div className="radar-segment" aria-label="快讯时间范围">
              {[12, 24, 72].map((value) => (
                <button type="button" key={value} className={wireHours === value ? 'active' : ''} aria-pressed={wireHours === value} onClick={() => setWireHours(value)}>
                  {value < 24 ? `${value}h` : `${value / 24}d`}
                </button>
              ))}
            </div>
            {wire && (
              <div className="wire-agent-status" aria-label="事件 Agent 状态">
                <span><strong>{wire.stats.published}</strong> 已发布</span>
                <span><strong>{wire.stats.pending}</strong> 待处理</span>
                <span><strong>{wire.stats.running}</strong> 核验中</span>
                {wire.stats.skipped > 0 && <span><strong>{wire.stats.skipped}</strong> 未发布</span>}
                {wire.stats.failed > 0 && <span className="failed"><strong>{wire.stats.failed}</strong> 失败</span>}
              </div>
            )}
            <button type="button" className="report-icon-button" onClick={() => void loadWire(true)} disabled={wireLoading} title="刷新快讯" aria-label="刷新快讯">
              <RefreshCw size={18} className={wireLoading ? 'spin' : ''} aria-hidden="true" />
            </button>
          </section>
          <section className="wire-feed" aria-label="事件快讯" aria-busy={wireLoading}>
            {wireError && <LoadError message={wireError} onRetry={() => void loadWire(true)} busy={wireLoading} />}
            {wireLoading && !wire ? (
              <RadarSkeleton label="正在更新" />
            ) : wire?.items.length ? wire.items.map((item) => (
              <article className={`wire-item status-${item.verification_status}`} key={item.event_id}>
                <time className="wire-time" dateTime={item.published_at}>{formatDate(item.published_at)}</time>
                <div className="wire-copy">
                  <h2>
                    <button type="button" className="wire-item-open" onClick={() => openWireEvent(item.event_id)}>{item.message}</button>
                  </h2>
                  <div>
                    <span className="wire-verification"><ShieldCheck size={13} aria-hidden="true" />{item.verification_label}</span>
                    <span>{item.cited_source_count}/{item.source_count} 源支撑</span>
                    <span>材料分 {item.verification_score}</span>
                  </div>
                  <small>{item.verification_reason}</small>
                </div>
                <ArrowRight size={17} aria-hidden="true" />
              </article>
            )) : (
              !wireError && <div className="radar-empty"><Radio aria-hidden="true" />暂无快讯</div>
            )}
          </section>
        </main>
      ) : (
        <main id="radar-main" className="watch-main" tabIndex={-1}>
          <h1 className="visually-hidden">关注</h1>
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
                    <h2>{watchDisplayName(watch.name)}</h2>
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
                  <button type="button" className="danger" onClick={() => void deleteWatch(watch)} disabled={saving} aria-label={`删除 ${watchDisplayName(watch.name)}`}>
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

// The card is an article so its title stays a navigable heading; only the title button is interactive,
// and its ::after overlay extends the hit area to the whole card.
const EventCard = memo(function EventCard({
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
  onSelect: (eventId: string, card: HTMLButtonElement) => void
}) {
  const stateClass = item.feedback === 'known' ? ' completed' : saved ? ' saved' : ''
  return (
    <article className={`event-card${active ? ' active' : ''}${stateClass}`}>
      <span className="event-index" aria-hidden="true">{String(index).padStart(2, '0')}</span>
      <div className="event-card-body">
        <div className="event-meta">
          <RevisionBadge type={item.revision_type} />
          {saved && <span className="workflow-state"><Bookmark size={13} aria-hidden="true" />稍后</span>}
          {item.feedback === 'known' && <span className="workflow-state"><CheckCircle2 size={13} aria-hidden="true" />已完成</span>}
          <span><Clock3 size={13} aria-hidden="true" /><time dateTime={item.published_at}>{formatDate(item.published_at)}</time></span>
          <span><ShieldCheck size={13} aria-hidden="true" />{item.confidence_label}</span>
          {item.editorial_score != null && <span title="按内容类型的评分标准给出的 0–10 分">评审 {item.editorial_score.toFixed(1)}</span>}
        </div>
        <h3>
          <button type="button" className="event-card-select" aria-current={active ? 'true' : undefined} onClick={(event) => onSelect(item.event_id, event.currentTarget)}>
            {item.title}
          </button>
        </h3>
        <p>{item.what_happened}</p>
        <SourceCoverage item={item} />
        <div className="event-card-footer">
          <span className="event-change"><TrendingUp size={14} aria-hidden="true" />{item.why_now}</span>
          {item.watch_names.map((watch) => <span className="watch-hit" key={watch}>命中 {watchDisplayName(watch)}</span>)}
        </div>
      </div>
      <ArrowRight className="event-open" size={18} aria-hidden="true" />
    </article>
  )
})

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
  const [inspectorView, setInspectorView] = useState<'overview' | 'coverage' | 'timeline'>('overview')
  const inspectorTabs = [['overview', '概览'], ['coverage', '报道对照'], ['timeline', '时间线']] as const
  return (
    <div className="event-inspector-content">
      <div className="inspector-heading">
        <RevisionBadge type={event.revision_type} />
        <span>{coverageLabel(event.independent_source_count)} · {detail.evidence.length} 条可读材料</span>
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
      <div className="inspector-tabs" role="tablist" aria-label="事件详情视图" onKeyDown={(e) => {
        const current = inspectorTabs.findIndex(([value]) => value === inspectorView)
        const next = e.key === 'ArrowRight' ? (current + 1) % 3 : e.key === 'ArrowLeft' ? (current + 2) % 3 : e.key === 'Home' ? 0 : e.key === 'End' ? 2 : -1
        if (next < 0) return
        e.preventDefault()
        setInspectorView(inspectorTabs[next][0])
        e.currentTarget.querySelectorAll<HTMLButtonElement>('[role="tab"]')[next]?.focus()
      }}>
        {inspectorTabs.map(([value, label]) => <button type="button" role="tab" key={value} id={`inspector-tab-${value}`} aria-selected={inspectorView === value} aria-controls={`inspector-panel-${value}`} tabIndex={inspectorView === value ? 0 : -1} onClick={() => setInspectorView(value)}>{label}{value === 'coverage' && <span>{detail.evidence.length}</span>}</button>)}
      </div>
      <div role="tabpanel" id="inspector-panel-overview" aria-labelledby="inspector-tab-overview" hidden={inspectorView !== 'overview'} tabIndex={0}>
      <section className="inspector-summary">
        <h3>发生了什么</h3>
        <p>{event.what_happened}</p>
        <h3>为什么现在值得看</h3>
        <p>{event.why_now}</p>
        <h3>影响与边界</h3>
        <p>{event.why_it_matters}</p>
        {event.history_note && (
          <>
            <h3>此前进展</h3>
            <p>{event.history_note}</p>
            {event.history_refs.length > 0 && (
              <ul className="inspector-refs">
                {event.history_refs.map((ref) => (
                  <li key={ref.event_id}><a href={`?event=${ref.event_id}`}>{ref.title}</a><small>{ref.digest_date}</small></li>
                ))}
              </ul>
            )}
          </>
        )}
        {event.takeaway && (
          <>
            <h3>方法与启示</h3>
            <p>{event.takeaway}</p>
          </>
        )}
        {event.community_note && (
          <>
            <h3>社区观点</h3>
            <p>{event.community_note}</p>
          </>
        )}
        {(event.editorial_score != null || event.editorial_tags.length > 0) && (
          <p className="inspector-editorial">
            {event.editorial_score != null && <span>评审 {event.editorial_score.toFixed(1)} / 10</span>}
            {event.editorial_tags.map((tag) => <span key={tag}>{tag}</span>)}
          </p>
        )}
      </section>
      <button type="button" className="coverage-entry" onClick={() => {
        setInspectorView('coverage')
        document.getElementById('inspector-tab-coverage')?.focus()
      }}>
        <BookOpen size={18} aria-hidden="true" /><span><strong>报道对照</strong><small>{event.independent_source_count === 1 ? '单一来源，仍需交叉核对' : `${coverageLabel(event.independent_source_count)} · ${detail.evidence.length} 条可读材料`}</small></span><ArrowRight size={17} aria-hidden="true" />
      </button>
      <OsintPanel key={event.event_id} eventId={event.event_id} />
      </div>
      <div role="tabpanel" id="inspector-panel-coverage" aria-labelledby="inspector-tab-coverage" hidden={inspectorView !== 'coverage'} tabIndex={0}>
        <EvidenceComparison detail={detail} onOpenArticle={onOpenArticle} />
      </div>
      <div role="tabpanel" id="inspector-panel-timeline" aria-labelledby="inspector-tab-timeline" hidden={inspectorView !== 'timeline'} tabIndex={0}>
      <section className="event-timeline">
        <div className="inspector-section-title"><Clock3 size={16} /><strong>事件时间线</strong></div>
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
      </div>

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

function LoadError({ message, onRetry, busy }: { message: string; onRetry: () => void; busy: boolean }) {
  return <div className="radar-load-error" role="alert">
    <XCircle size={20} aria-hidden="true" />
    <div><strong>加载失败</strong><p>{message}</p></div>
    <button type="button" onClick={onRetry} disabled={busy}><RefreshCw size={16} />重试</button>
  </div>
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
