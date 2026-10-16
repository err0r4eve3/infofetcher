import { useCallback, useEffect, useRef, useState } from 'react'
import { api, formatDate } from './api'
import { OsintEntities } from './OsintEntities'
import { OsintDocuments } from './OsintDocuments'
import { CaseDetail, Capture, ResearchReport, Review, RunBundle, caseLink, safeLink } from './osintTypes'

const TOOL_NAMES: Record<string, string> = {
  local_evidence: '本地材料', local_search: '站内检索', source_audit: '来源审计',
  text_comparison: '转载线索', timeline_audit: '时间核对', evidence_fragments: '证据片段',
  find_in_page: '正文定位', gap_check: '缺口检查', citation_audit: '引用校验',
}
const STOP_NAMES: Record<string, string> = { local_only: '本地分析结束', coverage_target: '已读取不同来源的相关正文', provider_unavailable: '搜索服务不可用', time_budget: '时间预算耗尽', search_budget: '搜索预算耗尽', page_budget: '读取预算耗尽', no_progress: '没有新增材料' }
const SOURCE_STATES: Record<string, string> = { discovered: '仅发现', read: '已读正文', failed: '读取失败', duplicate: '重复来源' }
const VERDICTS = { confirmed: '采纳', rejected: '驳回', needs_review: '待核查' }
const FRAGMENT_NAMES: Record<string, string> = { correction: '更正措辞', numeric: '数字线索', statement: '原文陈述' }

export function OsintResearch({ eventId, title = '', caseId }: { eventId?: string; title?: string; caseId?: string }) {
  const [query, setQuery] = useState(Array.from(title).slice(0, 240).join(''))
  const [mode, setMode] = useState<'online' | 'local'>('online')
  const [busy, setBusy] = useState(false)
  const [activeJob, setActiveJob] = useState(false)
  const [pendingJob, setPendingJob] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [archive, setArchive] = useState<CaseDetail | null>(null)
  const [run, setRun] = useState<RunBundle | null>(null)
  const [comparison, setComparison] = useState<ResearchReport | null>(null)
  const request = useRef<AbortController | null>(null)
  const report = run?.report
  const originEvent = archive?.case.event_id ?? eventId
  const eventAvailable = archive?.case.event_available ?? Boolean(eventId)
  const historyPath = caseId ? `/api/v2/osint/cases/${caseId}` : `/api/v2/events/${eventId}/osint/history`

  useEffect(() => {
    const controller = new AbortController()
    request.current = controller
    setLoading(true); setError('')
    void (async () => {
      const data = await api<CaseDetail | null>(historyPath, { signal: controller.signal })
      if (controller.signal.aborted) return
      setArchive(data)
      if (data) setQuery(Array.from(data.runs[0]?.query ?? data.case.title).slice(0, 240).join(''))
      const requestedRun = caseId ? new URLSearchParams(location.search).get('run') : null
      const runId = requestedRun ?? data?.runs[0]?.id
      if (data && runId) {
        const bundle = await api<RunBundle>(`/api/v2/osint/runs/${encodeURIComponent(runId)}`, { signal: controller.signal })
        if (bundle.case_id !== data.case.id) throw new Error('该调查版本不属于当前档案')
        if (!controller.signal.aborted) {
          setRun(bundle)
          if (!data.runs.some((r) => r.id === bundle.id)) setArchive({ ...data, runs: [{ id: bundle.id,
            created_at: bundle.report.generated_at, created_by: bundle.created_by, query: bundle.report.query,
            status: bundle.report.status, material_count: bundle.report.materials.length }, ...data.runs] })
        }
      }
    })().catch((e: unknown) => { if (!controller.signal.aborted) setError(String(e)) })
      .finally(() => { if (!controller.signal.aborted) setLoading(false) })
    return () => { controller.abort(); request.current?.abort() }
  }, [historyPath])

  const reloadRun = useCallback(async () => {
    if (!run) return
    const data = await api<RunBundle>(`/api/v2/osint/runs/${run.id}`)
    setRun((current) => current?.id === data.id ? data : current)
  }, [run])

  async function selectRun(id: string) {
    request.current?.abort()
    const controller = new AbortController(); request.current = controller
    setBusy(true); setError(''); setComparison(null)
    try {
      const data = await api<RunBundle>(`/api/v2/osint/runs/${id}`, { signal: controller.signal })
      if (!controller.signal.aborted) {
        setRun(data)
        if (caseId) { const url = new URL(location.href); url.searchParams.set('run', id); history.replaceState(history.state, '', url) }
      }
    } catch (e) { if (!controller.signal.aborted) setError(String(e)) }
    finally { if (!controller.signal.aborted) setBusy(false) }
  }

  const caseReady = useCallback(async (id: string) => {
    const data = await api<CaseDetail>(`/api/v2/osint/cases/${id}`)
    setArchive(data)
  }, [])
  const runReady = useCallback(async (id: string) => {
    const data = await api<RunBundle>(`/api/v2/osint/runs/${id}`)
    const archive = await api<CaseDetail>(`/api/v2/osint/cases/${data.case_id}`)
    setRun(data); setArchive(archive); setComparison(null)
    if (caseId) { const url = new URL(location.href); url.searchParams.set('run', id); history.replaceState(history.state, '', url) }
  }, [caseId])

  async function investigate() {
    if (!originEvent && !archive) return
    setBusy(true); setError(''); setComparison(null)
    try {
      const path = archive ? `/api/v2/osint/cases/${archive.case.id}/jobs` : `/api/v2/events/${originEvent}/osint/jobs`
      const result = await api<{ id: string; case_id: string }>(path, {
        method: 'POST', body: JSON.stringify({ query: query.trim(), mode }),
      })
      setPendingJob(result.id); setActiveJob(true)
      await caseReady(result.case_id)
    } catch (e) { setError(String(e)) }
    finally { setBusy(false) }
  }

  async function older() {
    if (!archive || archive.next_offset === null) return
    setBusy(true); setError('')
    try {
      const data = await api<CaseDetail>(`/api/v2/osint/cases/${archive.case.id}?offset=${archive.next_offset}`)
      setArchive((old) => old && ({ ...data,
        runs: [...old.runs, ...data.runs.filter((r) => !old.runs.some((item) => item.id === r.id))]
          .sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at) || b.id.localeCompare(a.id)),
        captures: [...old.captures, ...data.captures.filter((c) => !old.captures.some((item) => item.id === c.id))],
      }))
    } catch (e) { setError(String(e)) } finally { setBusy(false) }
  }

  const runIndex = archive?.runs.findIndex((r) => r.id === run?.id) ?? -1
  const previous = runIndex >= 0 ? archive?.runs.slice(runIndex + 1)
    .find((r) => Date.parse(r.created_at) <= Date.parse(report?.generated_at ?? '')) : undefined
  async function compare() {
    if (!previous) return
    setBusy(true); setError('')
    try { setComparison((await api<RunBundle>(`/api/v2/osint/runs/${previous.id}`)).report) }
    catch (e) { setError(String(e)) } finally { setBusy(false) }
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

  return <section className="osint-research" aria-label="调查档案">
    <h2>{caseId ? archive?.case.title ?? '调查档案' : '调查'}</h2>
    {error && <p role="alert">{error}</p>}
    {loading && <p role="status">正在读取调查档案…</p>}
    {!loading && !eventAvailable && archive?.case.origin !== 'topic' && <p>原事件已清理，已保存的材料、证据和复核记录仍可查阅。</p>}
    {!loading && (eventAvailable || archive) && <form onSubmit={(event) => { event.preventDefault(); void investigate() }}>
      <div className="osint-research-fields"><label>检索词<input value={query} required maxLength={240} disabled={busy || activeJob} onChange={(e) => setQuery(e.target.value)} /></label>
        <label>调查模式<select value={mode} disabled={busy || activeJob} onChange={(e) => setMode(e.target.value as 'online' | 'local')}><option value="online">联网调查</option><option value="local">仅本地材料</option></select></label>
      </div>
      {mode === 'online' && <p>联网检索会发送上述检索词及补查词，不发送本地正文或文件。</p>}
      <div className="osint-links"><button type="submit" disabled={busy || activeJob || !query.trim()}>{busy ? '正在排队…' : activeJob ? '调查进行中' : run ? '再次调查' : '开始调查'}</button>
        {archive && !caseId && <a href={caseLink(archive.case.id, run?.id)}>打开档案</a>}
      </div>
      {busy && <p role="status">正在冻结调查输入…</p>}
    </form>}
    {!loading && (archive || eventId) && <OsintDocuments caseId={archive?.case.id} eventId={eventId} pendingJob={pendingJob} hasRun={Boolean(run)} onCaseReady={caseReady} onRunReady={runReady} onActiveChange={setActiveJob} />}
    {archive && <>
      {archive.runs.length > 0 && <div className="osint-research-fields"><label>调查版本<select aria-label="调查版本" value={run?.id ?? ''} disabled={busy || loading} onChange={(e) => void selectRun(e.target.value)}>
        {!run && <option value="">选择版本</option>}
        {archive.runs.map((r) => <option key={r.id} value={r.id}>{formatDate(r.created_at)} · {r.material_count} 篇 · {r.id.slice(0, 8)}</option>)}
      </select></label></div>}
      {archive.next_offset !== null && <button disabled={busy} onClick={() => void older()}>加载更早记录</button>}
      <details className="osint-evidence-vault"><summary>原站证据 · {archive.captures.length}</summary>
        {archive.captures.length ? <CaptureList captures={archive.captures} /> : <p>暂无原站响应快照。正文分析快照保存在各调查版本中。</p>}
      </details>
    </>}
    {run && report && <div className="osint-research-result" aria-live="polite">
      <div className="osint-links"><strong>{report.status === 'insufficient' ? '证据不足' : report.mode === 'online' ? '调查结果' : '本地分析完成'} · 已保存</strong>
        <a href={`/api/v2/osint/runs/${run.id}/export`} download>导出调查</a>
        {previous && <button disabled={busy} onClick={() => void compare()}>对比上一版</button>}
      </div>
      <small>{formatDate(report.generated_at)} · {report.materials.length} / {report.total_sources} 篇材料 · {report.dependency_groups} 个依赖组</small>
      {comparison && <VersionDiff current={report} previous={comparison} />}
      {report.research && <div>
        <h3>研究状态</h3>
        <p>{STOP_NAMES[report.research.stop_reason] ?? report.research.stop_reason}</p>
        <small>搜索 {report.research.budget.searches_used} / {report.research.budget.searches_limit} 次 · 正文读取 {report.research.budget.pages_read} 篇 · 尝试 {report.research.budget.pages_attempted} / {report.research.budget.pages_limit} 次</small>
        {report.research.provider === 'gdelt' && <p>GDELT · 近一个月新闻检索，未覆盖全网及历史资料。</p>}
        <ul>{report.research.gaps.map((gap) => <li key={gap.kind}>{gap.message}</li>)}</ul>
        {report.research.rounds.length > 0 && <details><summary>检索轮次</summary><ol>{report.research.rounds.map((round, i) => <li key={i}><strong>{round.query}</strong><p>{round.reason} · {round.status === 'done' ? '完成' : '不可用'} · 新增 {round.candidates_added} 个候选、{round.pages_added} 篇正文</p></li>)}</ol></details>}
        {report.research.candidates.length > 0 && <details><summary>联网来源 · {report.research.candidates.length}</summary><ul>{report.research.candidates.map((candidate) => <li key={candidate.url}>
          <a href={safeLink(candidate.url)} target="_blank" rel="noreferrer">{candidate.title}</a><small>{SOURCE_STATES[candidate.status] ?? candidate.status} · 引用 {candidate.evidence_ids.length} 段</small>{candidate.message && <p>{candidate.message}</p>}
        </li>)}</ul></details>}
      </div>}
      <h3>待核对线索</h3>
      {report.findings.length === 0 ? <p>当前规则未发现线索，不代表材料已经证实。</p> : <ul>{report.findings.map((finding, i) => <li key={`${run.id}-${i}`}>
        <p>{finding.message}</p><ul>{sources(finding.article_ids)}</ul>
        <ReviewEditor runId={run.id} kind="finding" index={i} reviews={run.reviews} onSaved={reloadRun} />
      </li>)}</ul>}
      <details><summary>证据片段 · {report.fragments.length}</summary>
        <p>采纳或驳回只记录人工复核意见，不自动判定事实真伪。</p>
        {report.citation_audit && <p>已校验 {report.citation_audit.checked} 条原文引用的位置与快照完整性；未核验语义支持。</p>}
        {report.fragments.map((fragment, i) => <blockquote key={`${run.id}-${i}`}>
          <small>{FRAGMENT_NAMES[fragment.kind] ?? fragment.kind} · {fragment.dependency_groups} 个依赖组出现相同文字</small>
          <p>{fragment.excerpt}</p><ul>{sources(fragment.article_ids)}</ul>
          {fragment.evidence_ids && <details><summary>引用定位</summary><ul>{fragment.evidence_ids.map((id) => {
            const evidence = report.evidence?.find((e) => e.id === id)
            return evidence && <li key={id}><a href={`#snapshot-${run.id}-${evidence.article_id}`}>{evidence.source_title}</a><small>正文 UTF-8 字节 {evidence.start_byte}–{evidence.end_byte}</small><code>{id}</code></li>
          })}</ul></details>}
          <ReviewEditor runId={run.id} kind="fragment" index={i} reviews={run.reviews} onSaved={reloadRun} />
        </blockquote>)}
      </details>
      <details><summary>来源与时间</summary><ul>{timeline.map((m) => <li key={m.id} id={`snapshot-${run.id}-${m.id}`}>
        <a href={safeLink(m.url)} target="_blank" rel="noreferrer">{m.title}</a>
        <small>组 {m.dependency_group} · 源报发布时间：{m.reported_published_at ? formatDate(m.reported_published_at) : '未知'}<br />{m.document ? '导入时间' : '抓取时间'}：{formatDate(m.retrieved_at)}{m.content_truncated && ' · 正文已截断'}</small>
        {m.document && <small>导入文件 · 原件 SHA-256：<code>{m.document.sha256}</code> 页码与识别方式保存在文件提取记录中。</small>}
        {m.retrieval && <small>联网正文 · 响应 SHA-256：<code>{m.retrieval.response_sha256}</code></small>}
        <details><summary>正文快照</summary><p className="osint-snapshot-text">{m.content}</p><code>{m.text_sha256}</code></details>
      </li>)}</ul></details>
      {report.captures.length > 0 && <details><summary>本版引用的原站证据 · {report.captures.length}</summary><CaptureList captures={report.captures} /></details>}
      <OsintEntities key={run.id} run={run} onSaved={reloadRun} />
      <details><summary>站内相关材料 · {report.leads.length}</summary><p>检索词：{report.query}。结果仅作线索，不自动并入当前事件。</p><ul>{report.leads.map((lead) => <li key={lead.article_id}><a href={safeLink(lead.url)} target="_blank" rel="noreferrer">{lead.title}</a></li>)}</ul></details>
      <details><summary>调查过程</summary><ol>{report.steps.map((step, i) => <li key={i}><strong>{TOOL_NAMES[step.tool] ?? step.tool} · {step.status === 'done' ? '完成' : '不可用'}</strong><p>{step.message}</p></li>)}</ol>
        <small>执行人：{run.created_by}</small><p><code>{run.report_sha256}</code></p>
      </details>
      <ul className="osint-research-warnings">{report.warnings.map((warning) => <li key={warning}>{warning}</li>)}</ul>
    </div>}
  </section>
}

function CaptureList({ captures }: { captures: Capture[] }) {
  return <ul>{captures.map((c) => <li key={c.id}>
    <strong>{c.details.article_title ?? c.article_id}</strong><small>{formatDate(c.captured_at)} · HTTP {c.details.http_status ?? '未知'} · {c.byte_length.toLocaleString()} bytes</small>
    <p><code>{c.sha256}</code></p><a href={`/api/v2/osint/captures/${c.id}/export`} download>导出原始证据</a>
  </li>)}</ul>
}

function VersionDiff({ current, previous }: { current: ResearchReport; previous: ResearchReport }) {
  const added = current.materials.filter((m) => !previous.materials.some((old) => old.id === m.id))
  const removed = previous.materials.filter((m) => !current.materials.some((now) => now.id === m.id))
  const changed = current.materials.filter((m) => previous.materials.some((old) => old.id === m.id && old.full_text_sha256 !== m.full_text_sha256))
  const rows = [{ label: '新增', items: added }, { label: '本版未纳入', items: removed }, { label: '正文变化', items: changed }]
  return <div className="osint-version-diff"><h4>版本变化</h4><small>对比 {formatDate(previous.generated_at)}</small>
    <p>新增 {added.length} 篇 · 本版未纳入 {removed.length} 篇 · 正文变化 {changed.length} 篇</p>
    {rows.map(({ items, label }) => items.length > 0 && <ul key={label}>{items.map((m) => <li key={m.id}>{label}：{m.title}</li>)}</ul>)}
    <small>比较分析范围内的材料；未纳入可能由排序、静音或清理造成，不代表报道撤回。</small>
  </div>
}

function ReviewEditor({ runId, kind, index, reviews, onSaved }: { runId: string; kind: Review['target_kind']; index: number; reviews: Review[]; onSaved: () => Promise<void> }) {
  const history = reviews.filter((r) => r.target_kind === kind && r.target_index === index)
  const [verdict, setVerdict] = useState<Review['verdict']>(history[0]?.verdict ?? 'needs_review')
  const [note, setNote] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  async function save() {
    setBusy(true); setError('')
    try {
      await api(`/api/v2/osint/runs/${runId}/reviews`, { method: 'POST', body: JSON.stringify({ target_kind: kind, target_index: index, verdict, note }) })
      setNote(''); await onSaved()
    } catch (e) { setError(String(e)) } finally { setBusy(false) }
  }
  return <details className="osint-review"><summary>复核 · {history[0] ? VERDICTS[history[0].verdict] : '未复核'}</summary>
    {history[0] && <p>{history[0].note} <small>{history[0].actor} · {formatDate(history[0].created_at)}</small></p>}
    <form onSubmit={(e) => { e.preventDefault(); void save() }}>
      <div className="osint-research-fields"><label>复核结论<select value={verdict} onChange={(e) => setVerdict(e.target.value as Review['verdict'])}>{Object.entries(VERDICTS).map(([value, name]) => <option key={value} value={value}>{name}</option>)}</select></label>
        <label>复核理由<textarea value={note} required maxLength={2000} onChange={(e) => setNote(e.target.value)} /></label></div>
      <button disabled={busy || !note.trim()} type="submit">{busy ? '保存中…' : '保存复核'}</button>
    </form>
    {error && <p role="alert">{error}</p>}
    {history.length > 1 && <details><summary>复核历史 · {history.length}</summary><ol>{history.map((r) => <li key={r.id}>{VERDICTS[r.verdict]} · {r.note}<small>{r.actor} · {formatDate(r.created_at)}</small></li>)}</ol></details>}
  </details>
}
