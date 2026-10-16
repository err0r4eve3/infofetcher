import { ArrowLeft, RefreshCw, Search } from 'lucide-react'
import { useEffect, useState } from 'react'
import { api, formatDate } from './api'
import { OsintResearch } from './OsintResearch'
import { CaseSummary, caseLink } from './osintTypes'

export function OsintCasebook() {
  const [selected, setSelected] = useState(() => new URLSearchParams(location.search).get('case'))
  const [query, setQuery] = useState('')
  const [items, setItems] = useState<CaseSummary[]>([])
  const [next, setNext] = useState<number | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [refresh, setRefresh] = useState(0)
  const [topic, setTopic] = useState('')
  const [searching, setSearching] = useState(false)
  const [topicError, setTopicError] = useState('')
  useEffect(() => {
    const controller = new AbortController()
    setLoading(true); setError('')
    void api<{ items: CaseSummary[]; next_offset: number | null }>(`/api/v2/osint/cases?q=${encodeURIComponent(query)}`, { signal: controller.signal })
      .then((data) => { setItems(data.items); setNext(data.next_offset) })
      .catch((e: unknown) => { if (!controller.signal.aborted) setError(String(e)) })
      .finally(() => { if (!controller.signal.aborted) setLoading(false) })
    return () => controller.abort()
  }, [query, refresh])
  useEffect(() => {
    const listener = () => setSelected(new URLSearchParams(location.search).get('case'))
    window.addEventListener('popstate', listener)
    return () => window.removeEventListener('popstate', listener)
  }, [])
  async function more() {
    if (next === null) return
    setLoading(true); setError('')
    try {
      const data = await api<{ items: CaseSummary[]; next_offset: number | null }>(`/api/v2/osint/cases?q=${encodeURIComponent(query)}&offset=${next}`)
      setItems((old) => [...old, ...data.items.filter((item) => !old.some((o) => o.id === item.id))]); setNext(data.next_offset)
    } catch (e) { setError(String(e)) } finally { setLoading(false) }
  }
  async function searchTopic() {
    setSearching(true); setTopicError('')
    try {
      const result = await api<{ case_id: string }>('/api/v2/osint/topics', {
        method: 'POST', body: JSON.stringify({ query: topic.trim(), mode: 'online' }),
      })
      setTopic(''); select(result.case_id)
    } catch (e) { setTopicError(String(e)) } finally { setSearching(false) }
  }
  function select(id: string | null) {
    const url = new URL(location.href)
    url.searchParams.delete('run')
    if (id) url.searchParams.set('case', id)
    else { url.searchParams.delete('case'); setRefresh((value) => value + 1) }
    history.pushState(history.state, '', url); setSelected(id)
  }
  return <main id="radar-main" className="osint-casebook osint-panel" tabIndex={-1}>
    <h1 className="visually-hidden">调查</h1>
    {!selected && <form className="osint-topic-search" onSubmit={(e) => { e.preventDefault(); void searchTopic() }}>
      <label htmlFor="osint-topic">搜索事件</label>
      <div>
        <input id="osint-topic" value={topic} required maxLength={240} disabled={searching} placeholder="事件、人物或地点" onChange={(e) => setTopic(e.target.value)} />
        <button type="submit" disabled={searching || !topic.trim()}>{searching ? '正在排队…' : '搜索'}</button>
      </div>
      <small>检索站内文章和公开网页，结果存入档案。</small>
      {topicError && <p role="alert">{topicError}</p>}
    </form>}
    <div className="osint-case-heading">
      {selected
        ? <button type="button" className="osint-case-back" onClick={() => select(null)}><ArrowLeft size={16} aria-hidden="true" />返回档案</button>
        : <label className="osint-search"><Search size={16} aria-hidden="true" /><span className="visually-hidden">筛选档案</span><input type="search" placeholder="筛选档案" value={query} maxLength={240} onChange={(e) => setQuery(e.target.value)} /></label>}
      <button type="button" className="report-icon-button" onClick={() => setRefresh((v) => v + 1)} disabled={loading} title="刷新档案" aria-label="刷新档案">
        <RefreshCw size={18} className={loading ? 'spin' : ''} aria-hidden="true" />
      </button>
    </div>
    {selected ? <>
      <OsintResearch key={`${selected}:${refresh}`} caseId={selected} />
    </> : <>
      {error && <p role="alert">{error}</p>}
      {loading && <p role="status">正在读取档案…</p>}
      {!loading && items.length === 0 && <p>暂无调查档案。搜索一个事件，或打开事件的 OSINT 调查。</p>}
      <ul className="osint-case-list">{items.map((item) => <li key={item.id}>
        <a href={caseLink(item.id)} onClick={(e) => { if (!e.metaKey && !e.ctrlKey && !e.shiftKey && !e.altKey) { e.preventDefault(); select(item.id) } }}>{item.title}</a>
        <small>{item.run_count} 次调查 · {item.capture_count} 份原站证据 · {item.document_count ?? 0} 个文件 · {formatDate(item.updated_at)}{item.origin === 'topic' ? ' · 主动搜索' : !item.event_available && ' · 原事件已清理'}</small>
      </li>)}</ul>
      {next !== null && <button disabled={loading} onClick={() => void more()}>加载更多档案</button>}
    </>}
  </main>
}
