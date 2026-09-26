import { useEffect, useRef, useState } from 'react'
import { api, formatDate } from './api'

type Provider = 'local' | 'gdelt' | 'searxng'
type Capabilities = {
  analysis_available: boolean
  providers: { id: Provider; name: string; configured: boolean }[]
}
type ResearchReport = {
  event_id: string
  generated_at: string
  query: string
  status: 'grounded' | 'evidence_only' | 'insufficient'
  core_support_groups: number
  steps: { tool: string; status: string; message: string }[]
  leads: { title: string; url: string }[]
  materials: {
    id: string; title: string; url: string; origin: 'local' | 'web'; selected: boolean
    dependency_group: number; reported_published_at: string | null; retrieved_at: string
  }[]
  analysis: {
    summary: string; bullets: string[]; disagreements: string[]
    citations: { article_id: string; claim_id: string; excerpt: string; url: string; source_title: string }[]
  } | null
  warnings: string[]
}

const TOOL_NAMES: Record<string, string> = {
  local_evidence: '本地材料', search: '检索补证', read_page: '读取原文', source_audit: '来源审计', claim_verifier: '主张核验',
}
const STATUS_NAMES: Record<string, string> = { done: '完成', unavailable: '不可用', skipped: '跳过', insufficient: '证据不足' }

function safeLink(value: string): string | undefined {
  try {
    const url = new URL(value)
    return ['http:', 'https:'].includes(url.protocol) && !url.username && !url.password ? url.href : undefined
  } catch { return undefined }
}

export function OsintResearch({ eventId, title }: { eventId: string; title: string }) {
  const [capabilities, setCapabilities] = useState<Capabilities | null>(null)
  const [provider, setProvider] = useState<Provider>('local')
  const [query, setQuery] = useState(title.slice(0, 240))
  const [analyze, setAnalyze] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [report, setReport] = useState<ResearchReport | null>(null)
  const [retry, setRetry] = useState(0)
  const request = useRef<AbortController | null>(null)

  useEffect(() => {
    const controller = new AbortController()
    setError('')
    void api<Capabilities>('/api/v2/osint/tools', { signal: controller.signal }).then(setCapabilities)
      .catch((e: unknown) => { if (!controller.signal.aborted) setError(String(e)) })
    return () => { controller.abort(); request.current?.abort() }
  }, [retry])

  async function investigate() {
    const controller = new AbortController()
    request.current = controller
    setBusy(true)
    setError('')
    setReport(null)
    try {
      const result = await api<ResearchReport>(`/api/v2/events/${eventId}/osint/research`, {
        method: 'POST', signal: controller.signal,
        body: JSON.stringify({ provider, query: provider === 'local' ? undefined : query.trim(), analyze }),
      })
      if (!controller.signal.aborted) setReport(result)
    } catch (e) { if (!controller.signal.aborted) setError(String(e)) }
    finally { if (!controller.signal.aborted) setBusy(false) }
  }

  function downloadReport() {
    if (!report) return
    const url = URL.createObjectURL(new Blob([JSON.stringify(report, null, 2)], { type: 'application/json' }))
    const link = document.createElement('a')
    link.href = url
    link.download = `osint-research-${report.event_id}.json`
    link.click()
    setTimeout(() => URL.revokeObjectURL(url), 1000)
  }

  return <section className="osint-research" aria-label="调查 Agent">
    <h4>调查 Agent</h4>
    {error && <p role="alert">{error} {!capabilities && <button type="button" onClick={() => setRetry((n) => n + 1)}>重试</button>}</p>}
    {!capabilities && !error && <p role="status">正在加载工具…</p>}
    {capabilities && <form onSubmit={(event) => { event.preventDefault(); void investigate() }}>
      <div className="osint-research-fields">
        <label>检索工具<select value={provider} disabled={busy} onChange={(e) => setProvider(e.target.value as Provider)}>
          {capabilities.providers.map((p) => <option key={p.id} value={p.id} disabled={!p.configured}>{p.name}{!p.configured ? '（未配置）' : ''}</option>)}
        </select></label>
        {provider !== 'local' && <label>检索词<input value={query} required minLength={2} maxLength={240} disabled={busy} onChange={(e) => setQuery(e.target.value)} /></label>}
      </div>
      {provider !== 'local' && <p className="osint-note">将检索词发送给 {provider === 'gdelt' ? 'GDELT，检索近一个月的报道' : '配置的 SearXNG，补充更正与澄清检索'}，并读取最多 4 个公开原文页面。工具配置不代表当前可用。</p>}
      <label className="osint-archive-option"><input type="checkbox" checked={analyze} disabled={busy || !capabilities.analysis_available} onChange={(e) => setAnalyze(e.target.checked)} />主张核验{capabilities.analysis_available ? '（使用已配置模型）' : '（未配置模型）'}</label>
      <div className="osint-links"><button type="submit" disabled={busy || (provider !== 'local' && query.trim().length < 2)}>{busy ? '正在调查…' : '开始调查'}</button></div>
      {busy && <p role="status">正在整理来源和证据，最长约 3 分钟。</p>}
    </form>}
    {report && <div className="osint-research-result" aria-live="polite">
      <div className="osint-links"><strong>{report.status === 'grounded' ? '引文已核验' : report.status === 'insufficient' ? '证据不足' : '证据清单'}</strong><button type="button" onClick={downloadReport}>导出调查</button></div>
      <small>{formatDate(report.generated_at)} · {report.materials.filter((m) => m.selected).length} 篇分析材料</small>
      <p>{report.query}</p>
      {report.analysis && <>
        <p>{report.analysis.summary}</p>
        <small>核心主张：{report.core_support_groups} 个依赖组提供引文支持</small>
        <ul>{report.analysis.bullets.map((text, i) => <li key={i}>{text}</li>)}</ul>
        {report.analysis.disagreements.length > 0 && <><h4>分歧</h4><ul>{report.analysis.disagreements.map((text, i) => <li key={i}>{text}</li>)}</ul></>}
        <details><summary>核验引文</summary>{report.analysis.citations.map((c, i) => <blockquote key={`${c.article_id}-${i}`}><p>{c.excerpt}</p><a href={safeLink(c.url)} target="_blank" rel="noreferrer">{c.source_title}</a> · {c.claim_id === 'summary' ? '核心主张' : c.claim_id.startsWith('disagreement:') ? '分歧材料' : '补充主张'}</blockquote>)}</details>
      </>}
      <ul className="osint-research-warnings">{report.warnings.map((warning) => <li key={warning}>{warning}</li>)}</ul>
      <details><summary>调查过程</summary><ol>{report.steps.map((step, i) => <li key={i}><strong>{TOOL_NAMES[step.tool] ?? step.tool} · {STATUS_NAMES[step.status] ?? step.status}</strong><p>{step.message}</p></li>)}</ol></details>
      <details><summary>来源与时间</summary><ul>{report.materials.map((m) => <li key={m.id}>
        <a href={safeLink(m.url)} target="_blank" rel="noreferrer">{m.title}</a>
        <small>组 {m.dependency_group} · {m.origin === 'local' ? '本地材料' : '外部原文'} · {m.selected ? '已选取' : '未选取'}<br />
          源报发布时间：{m.reported_published_at ? formatDate(m.reported_published_at) : '未知'} · 读取时间：{formatDate(m.retrieved_at)}</small>
      </li>)}</ul></details>
      {report.leads.length > 0 && <details><summary>检索线索</summary><ul>{report.leads.map((lead) => <li key={lead.url}><a href={safeLink(lead.url)} target="_blank" rel="noreferrer">{lead.title}</a></li>)}</ul></details>}
    </div>}
  </section>
}
