import { useEffect, useRef, useState } from 'react'
import { api, formatDate } from './api'

type Document = { id: string; filename: string; status: string; sha256: string; byte_length: number; job_id: string; error: string | null }
type Job = { id: string; kind: string; status: string; attempts: number; error: string | null; run_id: string | null; created_at: string }
type Extraction = { extraction_sha256: string; extraction: { warning: string; pages: { page: number; method: string; text: string; sha256: string; truncated: boolean }[] } | null }
const STATUS: Record<string, string> = { queued: '排队中', running: '处理中', ready: '已提取', done: '完成', failed: '失败', cancelled: '已取消' }
const METHOD: Record<string, string> = { utf8: '文本', pdf_text: 'PDF 文字层', ocr: 'OCR' }
const active = (status: string) => status === 'queued' || status === 'running'

export function OsintDocuments({ caseId, eventId, pendingJob, hasRun, onCaseReady, onRunReady, onActiveChange }: {
  caseId?: string; eventId?: string; pendingJob: string | null; hasRun: boolean
  onCaseReady: (id: string) => Promise<void>; onRunReady: (id: string) => Promise<void>; onActiveChange: (active: boolean) => void
}) {
  const [documents, setDocuments] = useState<Document[]>([])
  const [jobs, setJobs] = useState<Job[]>([])
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [refresh, setRefresh] = useState(0)
  const [selected, setSelected] = useState<string | null>(null)
  const [extraction, setExtraction] = useState<Extraction | null>(null)
  const tracked = useRef(new Set<string>())
  const handled = useRef(new Set<string>())
  useEffect(() => {
    if (!caseId) return
    const controller = new AbortController()
    let timer: ReturnType<typeof setTimeout> | undefined
    async function poll() {
      try {
        const [docs, queue] = await Promise.all([
          api<{ items: Document[] }>(`/api/v2/osint/cases/${caseId}/documents`, { signal: controller.signal }),
          api<{ items: Job[] }>(`/api/v2/osint/cases/${caseId}/jobs`, { signal: controller.signal }),
        ])
        if (controller.signal.aborted) return
        setDocuments(docs.items); setJobs(queue.items)
        onActiveChange(queue.items.some((j) => j.kind === 'research' && active(j.status)))
        for (const job of queue.items) {
          if (active(job.status)) tracked.current.add(job.id)
          if (job.status === 'done' && job.run_id && (tracked.current.has(job.id) || job.id === pendingJob || !hasRun) && !handled.current.has(job.id)) {
            await onRunReady(job.run_id)
            handled.current.add(job.id)
            break
          }
        }
        if (controller.signal.aborted) return
        // A second tab can enqueue work, so idle views still refresh at a low rate.
        timer = setTimeout(() => void poll(), queue.items.some((j) => active(j.status)) ? 1500 : 10000)
      } catch (e) {
        if (!controller.signal.aborted) { setError(String(e)); timer = setTimeout(() => void poll(), 5000) }
      }
    }
    void poll()
    return () => { controller.abort(); if (timer) clearTimeout(timer) }
  }, [caseId, refresh, pendingJob, hasRun, onRunReady, onActiveChange])

  useEffect(() => {
    if (!selected) { setExtraction(null); return }
    const controller = new AbortController()
    setExtraction(null)
    void api<Extraction>(`/api/v2/osint/documents/${selected}`, { signal: controller.signal })
      .then(setExtraction).catch((e: unknown) => { if (!controller.signal.aborted) setError(String(e)) })
    return () => controller.abort()
  }, [selected])

  async function upload(file: File) {
    if (file.size === 0 || file.size > 10 * 1024 * 1024) { setError('文件须为 1 byte–10 MiB'); return }
    setBusy(true); setError('')
    try {
      const path = caseId ? `/api/v2/osint/cases/${caseId}/documents` : `/api/v2/events/${eventId}/osint/documents`
      const result = await api<{ case_id: string; job_id: string }>(`${path}?filename=${encodeURIComponent(file.name)}`, {
        method: 'POST', headers: { 'Content-Type': 'application/octet-stream' }, body: file,
      })
      tracked.current.add(result.job_id)
      await onCaseReady(result.case_id)
      setRefresh((v) => v + 1)
    } catch (e) { setError(String(e)) } finally { setBusy(false) }
  }
  async function action(id: string, name: 'cancel' | 'retry') {
    setBusy(true); setError('')
    try {
      await api(`/api/v2/osint/jobs/${id}/${name}`, { method: 'POST' })
      if (name === 'retry') { tracked.current.add(id); handled.current.delete(id) }
      setRefresh((v) => v + 1)
    } catch (e) { setError(String(e)) } finally { setBusy(false) }
  }
  async function openResult(id: string) {
    setBusy(true); setError('')
    try { await onRunReady(id) } catch (e) { setError(String(e)) } finally { setBusy(false) }
  }
  return <div className="osint-documents">
    <details open={documents.length > 0}><summary>本地文件 · {documents.length}</summary>
      <label className="osint-upload">导入文件<input type="file" accept=".pdf,.png,.jpg,.jpeg,.txt,text/plain,application/pdf,image/png,image/jpeg" disabled={busy || documents.length >= 20} onChange={(e) => { const file = e.target.files?.[0]; e.target.value = ''; if (file) void upload(file) }} /></label>
      <p className="osint-document-hint">PDF、PNG、JPEG、UTF-8 文本；每份档案最多 20 个文件，单文件 10 MiB、PDF 20 页。提取完成后再次调查即可纳入。</p>
      {busy && <p role="status">正在处理文件操作…</p>}
      <ul>{documents.map((doc) => <li key={doc.id}>
        <strong>{doc.filename}</strong><small>{STATUS[doc.status]} · {doc.byte_length.toLocaleString()} bytes</small>
        <code>{doc.sha256}</code>
        <div className="osint-links"><a href={`/api/v2/osint/documents/${doc.id}/original`} download>下载原件</a>
          {['failed', 'cancelled'].includes(doc.status) && <button disabled={busy} onClick={() => void action(doc.job_id, 'retry')}>重试提取</button>}
          {doc.status === 'ready' && <button onClick={() => setSelected(selected === doc.id ? null : doc.id)}>{selected === doc.id ? '收起文本' : '查看逐页文本'}</button>}
        </div>
        {doc.error && <p>{doc.error}</p>}
        {selected === doc.id && extraction?.extraction && <div className="osint-document-pages"><p>{extraction.extraction.warning}</p>
          {extraction.extraction.pages.map((page) => <details key={page.page}><summary>第 {page.page} 页 · {METHOD[page.method] ?? page.method}{page.truncated && ' · 已截断'}</summary>
            <pre className="osint-snapshot-text">{page.text}</pre><code>{page.sha256}</code>
          </details>)}
          <small>提取记录 SHA-256</small><code>{extraction.extraction_sha256}</code>
        </div>}
      </li>)}</ul>
    </details>
    {error && <p role="alert">{error}</p>}
    {jobs.length > 0 && <details open={jobs.some((j) => active(j.status) || j.status === 'failed')}><summary>后台任务 · {jobs.length}</summary>
      <p className="osint-document-hint">可以关闭页面。服务中断后任务会在租约到期（最多 10 分钟）后恢复；本地输入保持冻结，联网重试可能取得新版网页。</p>
      <ul>{jobs.map((job) => <li key={job.id}>
        <strong>{job.kind === 'document' ? '文件提取' : '调查'} · {STATUS[job.status]}</strong>
        <small>{formatDate(job.created_at)} · 执行 {job.attempts} 次 · {job.id.slice(0, 8)}</small>
        {job.error && <p>{job.error}</p>}
        <div className="osint-links">
          {active(job.status) && <button disabled={busy} onClick={() => void action(job.id, 'cancel')}>取消任务</button>}
          {['failed', 'cancelled'].includes(job.status) && <button disabled={busy} onClick={() => void action(job.id, 'retry')}>重试任务</button>}
          {job.run_id && <button disabled={busy} onClick={() => void openResult(job.run_id!)}>打开结果</button>}
        </div>
      </li>)}</ul>
    </details>}
  </div>
}
