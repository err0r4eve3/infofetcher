import { lazy, Suspense, useEffect, useRef, useState } from 'react'
import { api, formatDate } from './api'

// Research tooling is only needed after the dossier is opened, so it stays out of the report's first load.
const OsintResearch = lazy(() => import('./OsintResearch').then((module) => ({ default: module.OsintResearch })))

type Capture = {
  id: string
  captured_at: string
  sha256: string
  byte_length: number
  details: {
    final_url: string
    http_status: number
    outcome: 'captured' | 'http_error'
    page: {
      declared_canonical_url: string | null
      declared_published_at: string | null
      outbound_links: { url: string; label: string }[]
    } | null
    archive: { status: 'not_requested' | 'found' | 'not_found' | 'unavailable'; url?: string; timestamp?: string }
  }
}

type Dossier = {
  event_id: string
  title: string
  total_sources: number
  dependency_groups: number
  core_support_groups: number
  digest_message: string | null
  digest_completed_at: string | null
  sources: {
    article_id: string
    title: string
    source_title: string
    publisher: string
    url: string
    canonical_url: string
    content_hash: string
    reported_published_at: string | null
    fetched_at: string
    dependency_group: number
    latest_capture: Capture | null
  }[]
  citations: { claim_id: string; evidence_id: string; article_id: string; source_title: string; excerpt: string }[]
  disagreements: string[]
  gaps: string[]
}

function publicLink(value: string | null | undefined): string | undefined {
  try {
    const url = new URL(value ?? '')
    return ['https:', 'http:'].includes(url.protocol) && !url.username && !url.password ? url.href : undefined
  } catch { return undefined }
}

export function OsintPanel({ eventId }: { eventId: string }) {
  const [expanded, setExpanded] = useState(false)
  const [dossier, setDossier] = useState<Dossier | null>(null)
  const [error, setError] = useState('')
  const [reload, setReload] = useState(0)
  const [busy, setBusy] = useState<string | null>(null)
  const request = useRef<AbortController | null>(null)
  useEffect(() => {
    if (!expanded) return
    const controller = new AbortController()
    setError('')
    void api<Dossier>(`/api/v2/events/${eventId}/osint`, { signal: controller.signal })
      .then(setDossier).catch((e: unknown) => { if (!controller.signal.aborted) setError(String(e)) })
    return () => { controller.abort(); request.current?.abort() }
  }, [eventId, reload, expanded])

  async function capture(articleId: string) {
    const controller = new AbortController()
    request.current = controller
    setBusy(articleId)
    setError('')
    try {
      const result = await api<Capture>(`/api/v2/events/${eventId}/evidence/${articleId}/capture`, {
        method: 'POST', body: JSON.stringify({}), signal: controller.signal
      })
      setDossier((current) => current && ({ ...current, sources: current.sources.map((source) =>
        source.article_id === articleId ? { ...source, latest_capture: result } : source) }))
    } catch (e) { if (!controller.signal.aborted) setError(String(e)) }
    finally { if (!controller.signal.aborted) setBusy(null) }
  }

  return (
    <section className="osint-panel" aria-label="OSINT 调查">
      <details onToggle={(event) => event.currentTarget.open && setExpanded(true)}>
        <summary>OSINT 调查{dossier && <span>{dossier.dependency_groups} 个依赖组</span>}</summary>
        {dossier && <div className="osint-links"><button type="button" disabled={busy !== null} onClick={() => setReload((value) => value + 1)}>刷新调查</button></div>}
        {error && <p role="alert">{error} <button type="button" onClick={() => setReload((value) => value + 1)}>重试加载</button></p>}
        {!dossier && !error && <p role="status">正在整理证据…</p>}
        {dossier && <>
          <dl className="osint-stats">
            <div><dt>材料</dt><dd>{dossier.sources.length} / {dossier.total_sources}</dd></div>
            <div><dt>依赖组</dt><dd>{dossier.dependency_groups}</dd></div>
            <div><dt>核心主张支撑组</dt><dd>{dossier.core_support_groups}</dd></div>
          </dl>
          <h4>主张与不确定性</h4>
          {dossier.digest_message ? <>
            <p>{dossier.digest_message}</p>
            <small>引文定位于 {formatDate(dossier.digest_completed_at)} 的分析快照；不代表当前页面已复核。</small>
          </> : <p>暂无完成核验的快讯。可通过事件的“重新核验”生成主张与引文。</p>}
          {dossier.citations.map((citation, index) => <blockquote key={`${citation.claim_id}-${citation.evidence_id}-${index}`}>
            <p>{citation.excerpt}</p>
            <small>{citation.source_title} · {citation.claim_id === 'summary' ? '核心主张' : citation.claim_id.startsWith('disagreement:') ? '分歧材料' : '补充主张'}</small>
          </blockquote>)}
          {dossier.disagreements.length > 0 && <><h4>分歧</h4><ul>{dossier.disagreements.map((text, i) => <li key={i}>{text}</li>)}</ul></>}
          <h4>待核查</h4>
          <ul>{dossier.gaps.map((gap) => <li key={gap}>{gap}</li>)}</ul>
          <p>哪些原始记录可以支持或推翻该主张？是否把同一声明的转载当作独立报道？</p>
          <Suspense fallback={<p role="status">正在载入调查工具…</p>}>
            <OsintResearch key={eventId} eventId={eventId} title={dossier.title} />
          </Suspense>
          <h4>来源溯源</h4>
          <p>同发布者、同规范链接或相同正文指纹合并为一组；来源质量分仅用于筛选。</p>
          {dossier.sources.map((source) => <details className="osint-source" key={source.article_id}>
            <summary><span>组 {source.dependency_group} · {source.source_title}</span>{source.title}</summary>
            <dl>
              <dt>发布者标识</dt><dd>{source.publisher}</dd>
              <dt>源报发布时间</dt><dd>{source.reported_published_at ? formatDate(source.reported_published_at) : '未知'}</dd>
              <dt>入库抓取时间</dt><dd>{formatDate(source.fetched_at)}</dd>
              <dt>规范链接</dt><dd><a href={publicLink(source.canonical_url)} target="_blank" rel="noreferrer">{source.canonical_url}</a></dd>
            </dl>
            <div className="osint-links">
              <a href={publicLink(source.url)} target="_blank" rel="noreferrer">打开原文</a>
              <button type="button" disabled={busy !== null} onClick={() => void capture(source.article_id)}>{busy === source.article_id ? '正在取证…' : '抓取证据'}</button>
            </div>
            {source.latest_capture && <CaptureRecord capture={source.latest_capture} />}
          </details>)}
        </>}
      </details>
    </section>
  )
}

function CaptureRecord({ capture }: { capture: Capture }) {
  const { details } = capture
  return <div className="osint-capture" role="status">
    <p>{details.outcome === 'http_error' ? '原站返回错误页' : '响应已留存'} · HTTP {details.http_status} · {capture.byte_length.toLocaleString()} bytes</p>
    <small>{formatDate(capture.captured_at)}</small>
    <dl>
      <dt>最终地址</dt><dd>{details.final_url}</dd>
      <dt>SHA-256</dt><dd><code>{capture.sha256}</code></dd>
      {details.page?.declared_canonical_url && <><dt>页面声明的原始链接</dt><dd><a href={publicLink(details.page.declared_canonical_url)} target="_blank" rel="noreferrer">{details.page.declared_canonical_url}</a></dd></>}
      {details.page?.declared_published_at && <><dt>页面声明的发布时间</dt><dd>{details.page.declared_published_at}</dd></>}
    </dl>
    {details.page && details.page.outbound_links.length > 0 && <details>
      <summary>正文外链线索</summary>
      <ul>{details.page.outbound_links.map((link) => <li key={link.url}><a href={publicLink(link.url)} target="_blank" rel="noreferrer">{link.label || link.url}</a></li>)}</ul>
    </details>}
    <p>{details.archive.status === 'found' ? <a href={publicLink(details.archive.url)} target="_blank" rel="noreferrer">Wayback 存档 · {details.archive.timestamp} UTC</a>
      : details.archive.status === 'not_found' ? 'Wayback 未返回可用存档。' : details.archive.status === 'unavailable' ? 'Wayback 查询失败，存档状态未知。' : '未查询 Wayback。'}</p>
    <a href={`/api/v2/osint/captures/${capture.id}/export`} download>导出证据 JSON</a>
    <small className="osint-note">哈希用于核对留存内容是否一致，不证明陈述真实。记录独立留存，可在“调查”档案中查阅。</small>
  </div>
}
