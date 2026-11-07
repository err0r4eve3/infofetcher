import { ChevronDown, ChevronUp, ExternalLink, Languages, LoaderCircle, Sparkles, X } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { api, formatDate, type ArticleDetail, type ArticleTranslation } from './api'
import { formatArticleParagraphs } from './articleText'
import { shortSourceName } from './coverageModel'
import { SelectionTranslator } from './SelectionTranslator'
import './articleReader.css'

export type ReadingList = { ids: string[]; index: number }

// Reads one article over the report: the report keeps its place and scroll,
// and the sheet steps through the same evidence list the reader came from.
export function ArticleReader({ list, onClose, onNavigate, onOpenLibrary }: {
  list: ReadingList | null
  onClose: () => void
  onNavigate: (index: number) => void
  onOpenLibrary: (articleId: string) => void
}) {
  const ref = useRef<HTMLDialogElement>(null)
  const bodyRef = useRef<HTMLDivElement>(null)
  const progressRef = useRef<HTMLSpanElement>(null)
  const openerRef = useRef<HTMLElement | null>(null)
  const [closing, setClosing] = useState(false)
  const [article, setArticle] = useState<ArticleDetail | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [retry, setRetry] = useState(0)
  const [translating, setTranslating] = useState(false)
  const [translateError, setTranslateError] = useState<string | null>(null)
  const [showTranslation, setShowTranslation] = useState(false)
  const open = list !== null
  const articleId = list ? list.ids[list.index] : null

  useEffect(() => {
    const dialog = ref.current
    if (!dialog) return
    if (open && !dialog.open) {
      openerRef.current = document.activeElement as HTMLElement | null
      setClosing(false)
      dialog.showModal()
      bodyRef.current?.focus({ preventScroll: true })
    }
    if (!open && dialog.open) {
      if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) finishClose()
      else setClosing(true)
    }
  }, [open]) // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (!articleId) return
    const controller = new AbortController()
    setError(null)
    setShowTranslation(false)
    setTranslating(false)
    setTranslateError(null)
    bodyRef.current?.scrollTo({ top: 0 })
    api<ArticleDetail>(`/api/articles/${articleId}`, { signal: controller.signal })
      .then(setArticle)
      .catch((err) => {
        if (controller.signal.aborted) return
        setArticle(null)
        setError(err instanceof Error ? err.message : '文章读取失败')
      })
    return () => controller.abort()
  }, [articleId, retry])

  function finishClose() {
    ref.current?.close()
    setClosing(false)
    setArticle(null)
    const opener = openerRef.current
    openerRef.current = null
    if (opener?.isConnected) opener.focus({ preventScroll: true })
  }

  const updateProgress = useCallback(() => {
    const body = bodyRef.current
    const bar = progressRef.current
    if (!body || !bar) return
    const range = body.scrollHeight - body.clientHeight
    bar.style.transform = `scaleX(${range > 0 ? Math.min(1, body.scrollTop / range) : 0})`
  }, [])

  useEffect(updateProgress, [article, showTranslation, updateProgress])

  async function toggleTranslation() {
    if (!article) return
    if (article.translation?.status === 'done') {
      setShowTranslation((value) => !value)
      return
    }
    const id = article.id
    setTranslating(true)
    setTranslateError(null)
    try {
      const translation = await api<ArticleTranslation>('/api/translations', {
        method: 'POST',
        body: JSON.stringify({ article_id: id, target_language: 'zh-Hans' })
      })
      setArticle((current) => (current?.id === id ? { ...current, translation } : current))
      if (translation.status === 'done') setShowTranslation(true)
      else setTranslateError(translation.error || '翻译未完成，稍后再试')
    } catch (err) {
      setTranslateError(err instanceof Error ? err.message : '翻译失败')
    } finally {
      setTranslating(false)
    }
  }

  const loaded = article && article.id === articleId ? article : null
  const translated = Boolean(showTranslation && loaded?.translation?.status === 'done')
  const title = translated ? loaded?.translation?.translated_title || loaded?.title : loaded?.title
  const text = loaded
    ? translated
      ? loaded.translation?.translated_content || ''
      : loaded.extracted_content || loaded.content
    : ''
  const paragraphs = text ? formatArticleParagraphs(text) : []
  const extractionFailed = Boolean(loaded?.extraction_error && !loaded.extraction_error.includes('Feed 未提供正文'))
  const hasPrev = Boolean(list && list.index > 0)
  const hasNext = Boolean(list && list.index < list.ids.length - 1)

  return (
    <dialog
      ref={ref}
      className={`article-reader${closing ? ' closing' : ''}`}
      aria-label={loaded ? `阅读：${loaded.title}` : '阅读文章'}
      aria-busy={!loaded && !error}
      onCancel={(event) => { event.preventDefault(); onClose() }}
      onClick={(event) => { if (event.target === event.currentTarget) onClose() }}
      onAnimationEnd={(event) => { if (closing && event.target === event.currentTarget) finishClose() }}
      onKeyDown={(event) => {
        const target = event.target as HTMLElement
        if (event.metaKey || event.ctrlKey || event.altKey || target.closest('input, textarea, select')) return
        if ((event.key === 'j' || event.key === 'ArrowDown' && event.shiftKey) && hasNext && list) { event.preventDefault(); onNavigate(list.index + 1) }
        if ((event.key === 'k' || event.key === 'ArrowUp' && event.shiftKey) && hasPrev && list) { event.preventDefault(); onNavigate(list.index - 1) }
      }}
    >
      <div className="article-reader-bar">
        <button type="button" className="article-reader-icon" aria-label="关闭阅读" title="关闭（Esc）" onClick={onClose}><X size={18} aria-hidden="true" /></button>
        {list && list.ids.length > 1 && (
          <div className="article-reader-pager" role="group" aria-label="切换材料">
            <button type="button" className="article-reader-icon" aria-label="上一篇" title="上一篇（k）" disabled={!hasPrev} onClick={() => onNavigate(list.index - 1)}><ChevronUp size={18} aria-hidden="true" /></button>
            <span aria-live="polite">{list.index + 1} / {list.ids.length}</span>
            <button type="button" className="article-reader-icon" aria-label="下一篇" title="下一篇（j）" disabled={!hasNext} onClick={() => onNavigate(list.index + 1)}><ChevronDown size={18} aria-hidden="true" /></button>
          </div>
        )}
        <div className="article-reader-actions">
          <button
            type="button"
            aria-pressed={translated}
            disabled={!loaded || translating}
            onClick={() => void toggleTranslation()}
          >
            {translating ? <LoaderCircle size={15} className="spin" aria-hidden="true" /> : <Languages size={15} aria-hidden="true" />}
            {translating ? '翻译中' : translated ? '原文' : '译文'}
          </button>
          {loaded && <a href={loaded.url} target="_blank" rel="noreferrer"><ExternalLink size={15} aria-hidden="true" />来源</a>}
        </div>
        <span className="article-reader-progress" aria-hidden="true"><span ref={progressRef} /></span>
      </div>

      <div ref={bodyRef} className="article-reader-body" tabIndex={-1} onScroll={updateProgress}>
        {loaded ? (
          <article key={`${loaded.id}-${translated}`} className="article-reader-page">
            <p className="article-reader-meta">
              <strong title={loaded.feed_title}>{shortSourceName(loaded.feed_title) || '未标注来源'}</strong>
              <time dateTime={loaded.published_at ?? loaded.fetched_at}>{formatDate(loaded.published_at ?? loaded.fetched_at)}</time>
              {loaded.author && <span>{loaded.author}</span>}
            </p>
            <h1>{title}</h1>
            {translated && <p className="article-reader-note">机器翻译 · {loaded.translation?.model}</p>}
            {translateError && <p className="article-reader-note error" role="alert">{translateError}</p>}
            {!translated && loaded.summary?.status === 'done' && loaded.summary.summary && (
              <aside className="article-reader-summary">
                <Sparkles size={15} aria-hidden="true" />
                <p>{loaded.summary.summary}</p>
              </aside>
            )}
            <SelectionTranslator articleId={loaded.id} sourceLanguage={loaded.language} translated={translated}>
              {paragraphs.length ? paragraphs.map((paragraph, index) => <p key={index}>{paragraph}</p>)
                : <p className="article-reader-empty">没有可显示的正文。</p>}
            </SelectionTranslator>
            <footer className="article-reader-footer">
              {extractionFailed && (
                <a href={`https://www.removepaywall.com/search?url=${encodeURIComponent(loaded.url)}`} target="_blank" rel="noreferrer">正文抓取不完整，查公开存档</a>
              )}
              <a href={loaded.url} target="_blank" rel="noreferrer">打开来源网页</a>
              <button type="button" onClick={() => onOpenLibrary(loaded.id)}>在全部资料中打开</button>
            </footer>
            {hasNext && list && (
              <button type="button" className="article-reader-next" onClick={() => onNavigate(list.index + 1)}>
                下一篇<ChevronDown size={16} aria-hidden="true" />
              </button>
            )}
          </article>
        ) : error ? (
          <div className="article-reader-state" role="alert">
            <p>{error}</p>
            <button type="button" onClick={() => setRetry((value) => value + 1)}>重试</button>
          </div>
        ) : (
          <div className="article-reader-page article-reader-skeleton" role="status" aria-label="正在读取文章">
            <span /><span /><span /><span /><span /><span />
          </div>
        )}
      </div>
    </dialog>
  )
}
