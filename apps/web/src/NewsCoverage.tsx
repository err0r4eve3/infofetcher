import { ArrowRight, BookOpen, Layers3 } from 'lucide-react'
import { useMemo, useState } from 'react'
import { formatDate, type EventBriefItem, type EventDetail } from './api'
import { coverageLabel, evidenceForDisplay, evidenceTimestamp, sourceNames, type Evidence, type EvidenceOrder } from './coverageModel'

export function SourceCoverage({ item }: { item: EventBriefItem }) {
  const names = sourceNames(item.source_titles)
  return <div className="source-coverage">
    {names.length > 0 && <span className="source-monograms" aria-hidden="true">
      {names.slice(0, 3).map((name) => <span key={name}>{Array.from(name)[0].toLocaleUpperCase()}</span>)}
    </span>}
    <span className="source-names" title={names.join('、')}>{names.slice(0, 3).join('、') || '暂无来源信息'}{names.length > 3 ? ` 等 ${names.length} 个订阅源` : ''}</span>
    <span className="source-coverage-count">{coverageLabel(item.independent_source_count)}</span>
    {item.independent_source_count === 1 && <span className="single-source-label">单一来源</span>}
  </div>
}

export function EvidenceComparison({ detail, onOpenArticle }: {
  detail: EventDetail
  onOpenArticle: (id: string) => void
}) {
  const [source, setSource] = useState('')
  const [order, setOrder] = useState<EvidenceOrder>('latest')
  const sources = useMemo(() => sourceNames(detail.evidence.map((item) => item.source_title)), [detail.evidence])
  const effectiveSource = sources.includes(source) ? source : ''
  const evidence = useMemo(() => evidenceForDisplay(detail.evidence, effectiveSource, order), [detail.evidence, effectiveSource, order])
  const sourceCount = detail.event.independent_source_count
  const scope = detail.evidence_scope

  return <section className="coverage-comparison" aria-label="报道对照">
    <h3>来源覆盖</h3>
    <dl className="coverage-statistics">
      <div><dt><Layers3 size={15} aria-hidden="true" />来源组</dt><dd>{sourceCount > 0 ? sourceCount : '待确认'}</dd></div>
      <div><dt><BookOpen size={15} aria-hidden="true" />可读材料</dt><dd>{detail.evidence.length}</dd></div>
    </dl>
    <p className="coverage-note">来源组按站点或订阅通道去重，不等于独立核实。{scope ? `已载入 ${scope.returned_articles} / ${scope.total_articles} 条未静音材料，按质量选取最多 30 条。` : '材料数仅统计当前返回的内容，最多 30 条。'}链接站点和订阅名称不代表已核验的原始发布者。</p>
    {scope && <p className="coverage-note coverage-scope">当前材料中{scope.duplicate_groups > 0 ? `发现 ${scope.duplicate_groups} 组重复线索` : '尚未识别出重复线索'}。仅比对规范链接与已存正文指纹，未匹配不代表独立报道，转载方向仍待核对。</p>}
    {scope && (scope.feeds_with_failures > 0 || scope.feeds_never_succeeded > 0) && <p className="coverage-caution">当前材料涉及的订阅源：{[
      scope.feeds_with_failures > 0 ? `${scope.feeds_with_failures} 个有连续采集失败` : '',
      scope.feeds_never_succeeded > 0 ? `${scope.feeds_never_succeeded} 个暂无成功采集记录` : '',
    ].filter(Boolean).join('；')}。采集可能不完整，未收录不能视为未报道。</p>}
    {sourceCount === 1 && <p className="coverage-caution">当前仅有一个来源组，仍缺少其他来源的交叉核对。</p>}
    {sourceCount <= 0 && <p className="coverage-caution">来源覆盖暂未确认，请以具体材料为准。</p>}
    {detail.evidence.length > 0 ? <>
      <div className="comparison-toolbar">
        <label><span className="visually-hidden">按订阅源筛选报道</span>
          <select value={effectiveSource} onChange={(event) => setSource(event.target.value)}>
            <option value="">全部订阅源</option>
            {sources.map((name) => <option key={name} value={name}>{name}</option>)}
          </select>
        </label>
        <label><span className="visually-hidden">报道排序</span>
          <select value={order} onChange={(event) => setOrder(event.target.value as EvidenceOrder)}>
            <option value="latest">时间倒序</option>
            <option value="quality">材料质量</option>
          </select>
        </label>
      </div>
      <p className="comparison-count" role="status">显示 {evidence.length} 条材料</p>
      <div className="comparison-grid">
        {evidence.map((item) => <article className="comparison-article" key={item.article_id}>
          <header><span className="source-monogram" aria-hidden="true">{Array.from(item.source_title.trim())[0] || '?'}</span><strong>{item.source_title || '未标注来源'}</strong></header>
          <EvidenceTime item={item} />
          {item.provenance && <div className="evidence-provenance">
            <span>链接站点：{item.provenance.link_host || '未知'}</span>
            {item.provenance.link_is_aggregator && <span className="evidence-origin-unknown">聚合分发链接 · 原始发布者待确认</span>}
            {item.provenance.duplicate_group !== null && <span className="evidence-duplicate" title="同组材料存在规范链接或正文指纹匹配，无法据此判断谁先发布。">重复线索 · 组 {item.provenance.duplicate_group}</span>}
          </div>}
          <h4>{item.title}</h4>
          <p>{item.excerpt || '暂无正文摘录，可打开文章查看。'}</p>
          {item.collection && <details className="evidence-collection">
            <summary>采集记录{item.collection.consecutive_failures > 0 ? ` · 连续失败 ${item.collection.consecutive_failures} 次` : ''}</summary>
            <dl>
              <div><dt>订阅入口</dt><dd>{item.provenance?.feed_host || '未知'}</dd></div>
              <div><dt>本条收录</dt><dd>{item.fetched_at ? formatDate(item.fetched_at) : '未知'}</dd></div>
              <div><dt>最近成功</dt><dd>{item.collection.last_success_at ? formatDate(item.collection.last_success_at) : '暂无成功记录'}</dd></div>
            </dl>
          </details>}
          <footer><span title="材料质量分用于排序，不代表陈述真实">材料质量 {item.quality_score}</span>
            <button type="button" onClick={() => onOpenArticle(item.article_id)} aria-label={`阅读全文：${item.title}`}>阅读全文<ArrowRight size={14} aria-hidden="true" /></button>
          </footer>
        </article>)}
      </div>
    </> : <p className="inspector-empty">暂无可展示的原始材料。</p>}
  </section>
}

function EvidenceTime({ item }: { item: Evidence }) {
  const stamp = evidenceTimestamp(item)
  const label = stamp.kind === 'published' ? '发布于 ' : stamp.kind === 'collected' ? '发布时间未知 · 收录于 ' : '记录时间 '
  return <time dateTime={stamp.value}>{label}{stamp.value ? formatDate(stamp.value) : '未知'}</time>
}
