import { useEffect, useRef, useState } from 'react'
import { api, formatDate } from './api'

type ResearchReport = {
  event_id: string
  generated_at: string
  query: string
  status: 'analyzed' | 'insufficient'
  total_sources: number
  dependency_groups: number
  steps: { tool: string; status: string; message: string }[]
  leads: { article_id: string; title: string; url: string }[]
  materials: {
    id: string; title: string; url: string; dependency_group: number
    reported_published_at: string | null; retrieved_at: string; content_truncated: boolean
  }[]
  fragments: { kind: string; excerpt: string; numbers: string[]; article_ids: string[]; dependency_groups: number }[]
  findings: { kind: string; message: string; article_ids: string[] }[]
  warnings: string[]
}

const TOOL_NAMES: Record<string, string> = {
  local_evidence: '本地材料', local_search: '站内检索', source_audit: '来源审计',
  text_comparison: '转载线索', timeline_audit: '时间核对', evidence_fragments: '证据片段',
}
const FRAGMENT_NAMES: Record<string, string> = { correction: '更正措辞', numeric: '数字线索', statement: '原文陈述' }

function safeLink(value: string): string | undefined {
  try {
    const url = new URL(value)
    return ['http:', 'https:'].includes(url.protocol) && !url.username && !url.password ? url.href : undefined
  } catch { return undefined }
}

export function OsintResearch({ eventId, title }: { eventId: string; title: string }) {
  const [query, setQuery] = useState(Array.from(title).slice(0, 240).join(''))
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [report, setReport] = useState<ResearchReport | null>(null)
  const request = useRef<AbortController | null>(null)
  useEffect(() => () => request.current?.abort(), [])

  async function investigate() {
    const controller = new AbortController()
    request.current?.abort()
    request.current = controller
    setBusy(true)
    setError('')
    setReport(null)
    try {
      const result = await api<ResearchReport>(`/api/v2/events/${eventId}/osint/research`, {
        method: 'POST', signal: controller.signal, body: JSON.stringify({ query: query.trim() }),
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

  function sources(ids: string[]) {
    return ids.map((id) => {
      const source = report?.materials.find((m) => m.id === id)
      return source && <li key={id}><a href={safeLink(source.url)} target="_blank" rel="noreferrer">{source.title}</a> · 组 {source.dependency_group}</li>
    })
  }

  const timeline = report ? [...report.materials].sort((a, b) => {
    if (!a.reported_published_at) return b.reported_published_at ? 1 : 0
    if (!b.reported_published_at) return -1
    return a.reported_published_at.localeCompare(b.reported_published_at)
  }) : []

  return <section className="osint-research" aria-label="调查 Agent">
    <h4>调查 Agent</h4>
    <p>使用本地材料和站内索引进行规则分析，无需外部接口。</p>
    {error && <p role="alert">{error}</p>}
    <form onSubmit={(event) => { event.preventDefault(); void investigate() }}>
      <div className="osint-research-fields">
        <label>站内检索词<input value={query} required maxLength={240} disabled={busy} onChange={(e) => setQuery(e.target.value)} /></label>
      </div>
      <div className="osint-links"><button type="submit" disabled={busy || !query.trim()}>{busy ? '正在调查…' : '开始调查'}</button></div>
      {busy && <p role="status">正在核对来源、时间和正文片段…</p>}
    </form>
    {report && <div className="osint-research-result" aria-live="polite">
      <div className="osint-links"><strong>{report.status === 'insufficient' ? '证据不足' : '本地分析完成'}</strong><button type="button" onClick={downloadReport}>导出调查</button></div>
      <small>{formatDate(report.generated_at)} · {report.materials.length} / {report.total_sources} 篇材料 · {report.dependency_groups} 个依赖组</small>
      <h4>待核对线索</h4>
      {report.findings.length === 0 ? <p>当前规则未发现线索，不代表材料已经证实。</p> : <ul>{report.findings.map((finding, i) => <li key={i}>
        <p>{finding.message}</p><ul>{sources(finding.article_ids)}</ul>
      </li>)}</ul>}
      <details><summary>证据片段 · {report.fragments.length}</summary>
        <p>以下片段由规则抽取，未做语义核验。</p>
        {report.fragments.map((fragment, i) => <blockquote key={i}>
          <small>{FRAGMENT_NAMES[fragment.kind] ?? fragment.kind} · {fragment.dependency_groups} 个依赖组出现相同文字</small>
          <p>{fragment.excerpt}</p>
          {fragment.numbers.length > 0 && <small>数字：{fragment.numbers.join('、')}</small>}
          <ul>{sources(fragment.article_ids)}</ul>
        </blockquote>)}
      </details>
      <details><summary>来源与时间</summary><ul>{timeline.map((m) => <li key={m.id}>
        <a href={safeLink(m.url)} target="_blank" rel="noreferrer">{m.title}</a>
        <small>组 {m.dependency_group} · 源报发布时间：{m.reported_published_at ? formatDate(m.reported_published_at) : '未知'}<br />
          抓取时间：{formatDate(m.retrieved_at)}{m.content_truncated && ' · 正文分析范围已截断'}</small>
      </li>)}</ul></details>
      <details><summary>站内相关材料 · {report.leads.length}</summary><p>检索词：{report.query}。结果仅作线索，不自动并入当前事件。</p><ul>{report.leads.map((lead) => <li key={lead.article_id}><a href={safeLink(lead.url)} target="_blank" rel="noreferrer">{lead.title}</a></li>)}</ul></details>
      <details><summary>调查过程</summary><ol>{report.steps.map((step, i) => <li key={i}><strong>{TOOL_NAMES[step.tool] ?? step.tool} · {step.status === 'done' ? '完成' : '不可用'}</strong><p>{step.message}</p></li>)}</ol></details>
      <ul className="osint-research-warnings">{report.warnings.map((warning) => <li key={warning}>{warning}</li>)}</ul>
    </div>}
  </section>
}
