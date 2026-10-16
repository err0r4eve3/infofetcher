import { useEffect, useState } from 'react'
import { api, formatDate } from './api'
import { Entity, Material, Mention, RunBundle, caseLink } from './osintTypes'

const KINDS = { person: '人物', organization: '机构', location: '地点' }
const STATES = { suggested: '待确认', confirmed: '已确认', rejected: '已驳回' }
type EntityDetail = { entity: Entity; mentions: Mention[]; next_offset: number | null }

export function OsintEntities({ run, onSaved }: { run: RunBundle; onSaved: () => Promise<void> }) {
  const [kind, setKind] = useState<Entity['kind']>('organization')
  const [name, setName] = useState('')
  const [aliases, setAliases] = useState('')
  const [catalog, setCatalog] = useState<Entity[]>([])
  const [existing, setExisting] = useState('')
  const [articleId, setArticleId] = useState(run.report.materials[0]?.id ?? '')
  const [excerpt, setExcerpt] = useState(run.report.materials[0]?.content.slice(0, 250) ?? '')
  const [subject, setSubject] = useState('')
  const [object, setObject] = useState('')
  const [predicate, setPredicate] = useState('')
  const [relationArticle, setRelationArticle] = useState(run.report.materials[0]?.id ?? '')
  const [relationExcerpt, setRelationExcerpt] = useState(run.report.materials[0]?.content.slice(0, 250) ?? '')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const [focused, setFocused] = useState<string | null>(null)
  const confirmed = [...new Map(run.mentions.filter((m) => m.status === 'confirmed').map((m) => [m.entity.id, m.entity])).values()]

  useEffect(() => {
    const controller = new AbortController()
    const timer = setTimeout(() => {
      void api<{ items: Entity[] }>(`/api/v2/osint/entities?q=${encodeURIComponent(name.trim())}`, { signal: controller.signal })
        .then((data) => { setCatalog(data.items); setExisting('') })
        .catch((e: unknown) => { if (!controller.signal.aborted) setError(String(e)) })
    }, 200)
    return () => { clearTimeout(timer); controller.abort() }
  }, [name])

  async function saveEntity() {
    setBusy(true); setError(''); setNotice('')
    try {
      const identity = existing ? { entity_id: existing } : { kind, name, aliases: aliases.split(/[\n,，]/).map((s) => s.trim()).filter(Boolean) }
      const result = await api<{ entity_id: string }>(`/api/v2/osint/runs/${run.id}/entities`, {
        method: 'POST', body: JSON.stringify({ ...identity, article_id: articleId, excerpt }),
      })
      await onSaved(); setFocused(result.entity_id); setNotice('实体标注已保存')
    } catch (e) { setError(String(e)) } finally { setBusy(false) }
  }
  async function saveRelation() {
    setBusy(true); setError(''); setNotice('')
    try {
      await api(`/api/v2/osint/runs/${run.id}/relations`, { method: 'POST', body: JSON.stringify({ subject_id: subject, object_id: object, predicate, article_id: relationArticle, excerpt: relationExcerpt }) })
      await onSaved(); setNotice('关系记录已保存'); setPredicate('')
    } catch (e) { setError(String(e)) } finally { setBusy(false) }
  }
  return <section className="osint-entities" aria-label="实体关联">
    <h3>实体关联</h3>
    {error && <p role="alert">{error}</p>}{notice && <p role="status">{notice}</p>}
    {run.mentions.length === 0 ? <p>尚未标注人物、机构或地点。</p> : <ul className="osint-entity-list">{run.mentions.map((m) => <li key={m.id}>
      <button type="button" onClick={() => setFocused(m.entity.id)}>{m.entity.name}</button> <small>{KINDS[m.entity.kind]} · {STATES[m.status]}</small>
      <p>{m.excerpt}</p><small>{run.report.materials.find((a) => a.id === m.article_id)?.title}</small>
      <DecisionEditor path={`/api/v2/osint/mentions/${m.id}/review`} status={m.status} onSaved={onSaved} />
    </li>)}</ul>}
    {run.report.materials.length > 0 && <details><summary>标注实体</summary>
      <form onSubmit={(e) => { e.preventDefault(); void saveEntity() }}>
        <div className="osint-research-fields">
          <label>名称或别名<input value={name} required={!existing} maxLength={120} onChange={(e) => setName(e.target.value)} /></label>
          <label>关联身份<select value={existing} onChange={(e) => setExisting(e.target.value)}><option value="">新建实体</option>{catalog.map((e) => <option key={e.id} value={e.id}>{KINDS[e.kind]} · {e.name} · {e.id.slice(0, 8)}</option>)}</select></label>
          {!existing && <><label>实体类型<select value={kind} onChange={(e) => setKind(e.target.value as Entity['kind'])}>{Object.entries(KINDS).map(([key, value]) => <option value={key} key={key}>{value}</option>)}</select></label>
            <label>别名<input value={aliases} maxLength={1500} placeholder="以逗号分隔" onChange={(e) => setAliases(e.target.value)} /></label></>}
        </div>
        <EvidenceFields materials={run.report.materials} articleId={articleId} excerpt={excerpt} onArticle={setArticleId} onExcerpt={setExcerpt} />
        <p className="osint-note">同名可能指向不同对象。请核对身份后选择已有实体；别名用于发现候选关联。</p>
        <button type="submit" disabled={busy || (!existing && name.trim().length < 2) || !excerpt.trim()}>保存实体</button>
      </form>
    </details>}
    {run.relations.length > 0 && <div className="osint-relations"><h4>关系记录</h4>{run.relations.map((r) => <article key={r.id}>
      <p><button onClick={() => setFocused(r.subject_id)}>{r.subject_name}</button> <span>— {r.predicate} →</span> <button onClick={() => setFocused(r.object_id)}>{r.object_name}</button></p>
      <small>{STATES[r.status]} · 人工记录</small><blockquote>{r.excerpt}</blockquote>
      <small>{run.report.materials.find((a) => a.id === r.article_id)?.title}</small>
      <DecisionEditor path={`/api/v2/osint/relations/${r.id}/review`} status={r.status} onSaved={onSaved} />
    </article>)}</div>}
    {confirmed.length >= 2 && <details><summary>记录关系</summary><form onSubmit={(e) => { e.preventDefault(); void saveRelation() }}>
      <div className="osint-research-fields"><label>主体<select required value={subject} onChange={(e) => setSubject(e.target.value)}><option value="">选择实体</option>{confirmed.map((e) => <option key={e.id} value={e.id}>{e.name}</option>)}</select></label>
        <label>关系<input required value={predicate} maxLength={80} placeholder="例如：任职于" onChange={(e) => setPredicate(e.target.value)} /></label>
        <label>客体<select required value={object} onChange={(e) => setObject(e.target.value)}><option value="">选择实体</option>{confirmed.filter((e) => e.id !== subject).map((e) => <option key={e.id} value={e.id}>{e.name}</option>)}</select></label></div>
      <EvidenceFields materials={run.report.materials} articleId={relationArticle} excerpt={relationExcerpt} onArticle={setRelationArticle} onExcerpt={setRelationExcerpt} />
      <button disabled={busy || !subject || !object || subject === object || !predicate.trim()} type="submit">保存关系</button>
    </form></details>}
    {focused && <EntityExplorer key={focused} id={focused} onClose={() => setFocused(null)} onSaved={onSaved} />}
    {run.entity_audit.length > 0 && <details><summary>关联操作记录 · {run.entity_audit.length}</summary><ol>{run.entity_audit.map((a) => <li key={a.id}>
      {({ entity_linked: '标注实体', alias_candidate: '发现别名候选', mention_reviewed: '复核实体标注', relation_added: '记录关系', relation_reviewed: '复核关系' } as Record<string, string>)[a.action] ?? a.action}
      <small>{a.actor} · {formatDate(a.created_at)}{typeof a.details.note === 'string' && ` · ${a.details.note}`}</small>
    </li>)}</ol></details>}
  </section>
}

function EvidenceFields({ materials, articleId, excerpt, onArticle, onExcerpt }: { materials: Material[]; articleId: string; excerpt: string; onArticle: (id: string) => void; onExcerpt: (s: string) => void }) {
  const selected = materials.find((m) => m.id === articleId)
  return <>
    <div className="osint-research-fields"><label>依据材料<select value={articleId} required onChange={(e) => {
      onArticle(e.target.value); onExcerpt(materials.find((m) => m.id === e.target.value)?.content.slice(0, 250) ?? '')
    }}>{materials.map((m) => <option key={m.id} value={m.id}>{m.title}</option>)}</select></label>
      <label>原文片段<textarea value={excerpt} required maxLength={1000} onChange={(e) => onExcerpt(e.target.value)} /></label></div>
    {selected && <details><summary>查看保存正文</summary><p className="osint-snapshot-text">{selected.content}</p></details>}
  </>
}

function DecisionEditor({ path, status, onSaved }: { path: string; status: string; onSaved: () => Promise<void> }) {
  const [note, setNote] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  async function save(next: string) {
    setBusy(true); setError('')
    try { await api(path, { method: 'POST', body: JSON.stringify({ status: next, note }) }); setNote(''); await onSaved() }
    catch (e) { setError(String(e)) } finally { setBusy(false) }
  }
  return <details><summary>复核关联</summary><label className="osint-review-note">关联复核理由<textarea value={note} maxLength={2000} onChange={(e) => setNote(e.target.value)} /></label>
    <div className="osint-links">{status !== 'confirmed' && <button disabled={busy || !note.trim()} onClick={() => void save('confirmed')}>确认关联</button>}
      {status !== 'rejected' && <button disabled={busy || !note.trim()} onClick={() => void save('rejected')}>驳回关联</button>}</div>
    {error && <p role="alert">{error}</p>}
  </details>
}

function EntityExplorer({ id, onClose, onSaved }: { id: string; onClose: () => void; onSaved: () => Promise<void> }) {
  const [data, setData] = useState<EntityDetail | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const [cursor, setCursor] = useState<string | null>(null)
  useEffect(() => {
    const controller = new AbortController()
    void api<EntityDetail>(`/api/v2/osint/entities/${id}`, { signal: controller.signal }).then(setData)
      .catch((e: unknown) => { if (!controller.signal.aborted) setError(String(e)) })
    return () => controller.abort()
  }, [id])
  async function reload() { setData(await api<EntityDetail>(`/api/v2/osint/entities/${id}`)); await onSaved() }
  async function scan(before: string | null) {
    setBusy(true); setError('')
    try {
      const result = await api<{ scanned_runs: number; added: number; next_before: string | null }>(`/api/v2/osint/entities/${id}/scan`, { method: 'POST', body: JSON.stringify({ before }) })
      setCursor(result.next_before); setNotice(`已检查 ${result.scanned_runs} 个调查版本，新增 ${result.added} 条候选。${result.next_before ? '还有更早版本待检索。' : '本轮检索完成。'}`)
      await reload()
    } catch (e) { setError(String(e)) } finally { setBusy(false) }
  }
  async function more() {
    if (data?.next_offset === null || !data) return
    setBusy(true); setError('')
    try {
      const next = await api<EntityDetail>(`/api/v2/osint/entities/${id}?offset=${data.next_offset}`)
      setData((old) => old && ({ ...next, mentions: [...old.mentions, ...next.mentions] }))
    } catch (e) { setError(String(e)) } finally { setBusy(false) }
  }
  return <aside className="osint-entity-explorer" aria-label="实体档案">
    <div className="osint-case-heading"><h4>{data?.entity.name ?? '实体档案'}</h4><button onClick={onClose}>收起实体档案</button></div>
    {error && <p role="alert">{error}</p>}
    {data && <><p>{KINDS[data.entity.kind]} · {data.entity.aliases.join(' / ')}</p>
      <div className="osint-links"><button disabled={busy} onClick={() => void scan(null)}>查找关联</button>{cursor && <button disabled={busy} onClick={() => void scan(cursor)}>继续检索</button>}</div>
      <p>检索已保存正文中的名称和别名；候选命中不证明身份相同。</p>{notice && <p role="status">{notice}</p>}
      <ul>{data.mentions.map((m) => <li key={m.id}><a href={caseLink(m.case_id!, m.run_id)}>{m.case_title}</a> · {STATES[m.status]}<p>{m.excerpt}</p>
        <DecisionEditor path={`/api/v2/osint/mentions/${m.id}/review`} status={m.status} onSaved={reload} /></li>)}</ul>
      {data.next_offset !== null && <button disabled={busy} onClick={() => void more()}>加载更多关联</button>}
    </>}
  </aside>
}
