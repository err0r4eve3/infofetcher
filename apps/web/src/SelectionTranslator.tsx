import { Languages, LoaderCircle, RefreshCw, X } from 'lucide-react'
import {
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useRef,
  useState
} from 'react'
import { createPortal } from 'react-dom'
import { api, type SelectionTranslation } from './api'

const MAX_SELECTION_CHARS = 4_000
const POPOVER_WIDTH = 360
const POPOVER_ESTIMATED_HEIGHT = 220
const VIEWPORT_GAP = 12

type SelectionPhase = 'ready' | 'loading' | 'done' | 'error'

type SelectionState = {
  text: string
  characterCount: number
  left: number
  top: number
  placement: 'above' | 'below'
  phase: SelectionPhase
  translation?: string
  error?: string
}

type SelectionTranslatorProps = {
  articleId: string
  sourceLanguage: string | null
  translated: boolean
  children: ReactNode
}

function selectionPosition(range: Range) {
  const rects = Array.from(range.getClientRects())
  const rect = rects.at(-1) ?? range.getBoundingClientRect()
  const availableWidth = Math.max(0, window.innerWidth - VIEWPORT_GAP * 2)
  const halfWidth = Math.min(POPOVER_WIDTH, availableWidth) / 2
  const left = Math.min(
    window.innerWidth - VIEWPORT_GAP - halfWidth,
    Math.max(VIEWPORT_GAP + halfWidth, rect.left + rect.width / 2)
  )
  const canPlaceBelow = rect.bottom + VIEWPORT_GAP + POPOVER_ESTIMATED_HEIGHT < window.innerHeight
  return {
    left,
    top: canPlaceBelow ? rect.bottom + VIEWPORT_GAP : Math.max(VIEWPORT_GAP, rect.top - VIEWPORT_GAP),
    placement: canPlaceBelow ? 'below' as const : 'above' as const
  }
}

function selectionTargetLanguage(text: string, sourceLanguage: string | null) {
  const hasCjk = /[\u3400-\u9fff]/u.test(text)
  const hasLatin = /[a-z]/iu.test(text)
  if (hasCjk && !hasLatin) return 'en' as const
  if (hasLatin) return 'zh-Hans' as const
  return sourceLanguage?.toLowerCase().startsWith('zh') ? 'en' as const : 'zh-Hans' as const
}

function selectionSourceLanguage(text: string) {
  if (/[\u3040-\u30ff]/u.test(text)) return 'ja'
  if (/[\u1100-\u11ff\uac00-\ud7af]/u.test(text)) return 'ko'
  const hasCjk = /[\u3400-\u9fff]/u.test(text)
  const hasLatin = /[a-z]/iu.test(text)
  if (hasCjk && !hasLatin) return 'zh-Hans'
  if (hasLatin && !hasCjk) return 'en'
  return null
}

export function SelectionTranslator({
  articleId,
  sourceLanguage,
  translated,
  children
}: SelectionTranslatorProps) {
  const rootRef = useRef<HTMLDivElement | null>(null)
  const popoverRef = useRef<HTMLDivElement | null>(null)
  const requestRef = useRef<AbortController | null>(null)
  const cacheRef = useRef(new Map<string, string>())
  const [selection, setSelection] = useState<SelectionState | null>(null)

  const close = useCallback(() => {
    requestRef.current?.abort()
    requestRef.current = null
    setSelection(null)
  }, [])

  const captureSelection = useCallback(() => {
    if (translated) {
      close()
      return
    }
    const browserSelection = window.getSelection()
    const root = rootRef.current
    if (!browserSelection || browserSelection.isCollapsed || browserSelection.rangeCount === 0 || !root) {
      close()
      return
    }
    const range = browserSelection.getRangeAt(0)
    if (!root.contains(range.commonAncestorContainer)) {
      close()
      return
    }
    const text = browserSelection.toString().trim()
    if (!text) {
      close()
      return
    }
    const characterCount = Array.from(text).length
    setSelection({
      text,
      characterCount,
      ...selectionPosition(range),
      phase: characterCount > MAX_SELECTION_CHARS ? 'error' : 'ready',
      error: characterCount > MAX_SELECTION_CHARS
        ? `选区过长，单次最多 ${MAX_SELECTION_CHARS.toLocaleString()} 字符`
        : undefined
    })
  }, [close, translated])

  useEffect(() => close(), [articleId, close, translated])

  useEffect(() => {
    if (!selection) return
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') close()
    }
    const handlePointerDown = (event: PointerEvent) => {
      const target = event.target as Node
      if (!rootRef.current?.contains(target) && !popoverRef.current?.contains(target)) close()
    }
    const handleScroll = (event: Event) => {
      if (!popoverRef.current?.contains(event.target as Node)) close()
    }
    window.addEventListener('keydown', handleKeyDown)
    window.addEventListener('pointerdown', handlePointerDown)
    window.addEventListener('resize', close)
    window.addEventListener('scroll', handleScroll, true)
    return () => {
      window.removeEventListener('keydown', handleKeyDown)
      window.removeEventListener('pointerdown', handlePointerDown)
      window.removeEventListener('resize', close)
      window.removeEventListener('scroll', handleScroll, true)
    }
  }, [close, selection])

  async function translateSelection() {
    if (!selection || selection.characterCount > MAX_SELECTION_CHARS) return
    const text = selection.text
    const targetLanguage = selectionTargetLanguage(text, sourceLanguage)
    const requestSourceLanguage = selectionSourceLanguage(text)
    const cacheKey = `${requestSourceLanguage ?? ''}\u0000${targetLanguage}\u0000${text}`
    const cached = cacheRef.current.get(cacheKey)
    if (cached) {
      setSelection((current) => current?.text === text
        ? { ...current, phase: 'done', translation: cached, error: undefined }
        : current)
      return
    }

    requestRef.current?.abort()
    const controller = new AbortController()
    requestRef.current = controller
    setSelection((current) => current?.text === text
      ? { ...current, phase: 'loading', error: undefined }
      : current)
    try {
      const result = await api<SelectionTranslation>('/api/translations/selection', {
        method: 'POST',
        signal: controller.signal,
        body: JSON.stringify({
          text,
          source_language: requestSourceLanguage,
          target_language: targetLanguage
        })
      })
      cacheRef.current.set(cacheKey, result.translation)
      setSelection((current) => current?.text === text
        ? { ...current, phase: 'done', translation: result.translation, error: undefined }
        : current)
    } catch (error) {
      if (controller.signal.aborted) return
      setSelection((current) => current?.text === text
        ? {
            ...current,
            phase: 'error',
            error: error instanceof Error ? error.message : '划词翻译失败'
          }
        : current)
    } finally {
      if (requestRef.current === controller) requestRef.current = null
    }
  }

  function scheduleCapture(
    event: ReactPointerEvent<HTMLDivElement> | ReactKeyboardEvent<HTMLDivElement>
  ) {
    if (popoverRef.current?.contains(event.target as Node)) return
    window.requestAnimationFrame(captureSelection)
  }

  const popoverStyle = selection
    ? ({ left: selection.left, top: selection.top } satisfies CSSProperties)
    : undefined
  const targetLanguage = selection
    ? selectionTargetLanguage(selection.text, sourceLanguage)
    : 'zh-Hans'

  return (
    <>
      <div
        ref={rootRef}
        className="article-body"
        data-translated={translated || undefined}
        onPointerUp={scheduleCapture}
        onKeyUp={scheduleCapture}
      >
        {children}
      </div>
      {selection && typeof document !== 'undefined' && createPortal(
        <div
          ref={popoverRef}
          className="selection-translation"
          data-placement={selection.placement}
          data-phase={selection.phase}
          style={popoverStyle}
          role={selection.phase === 'ready' ? 'toolbar' : 'dialog'}
          aria-label="划词翻译"
          aria-live="polite"
        >
          {selection.phase === 'ready' ? (
            <button
              type="button"
              className="selection-translation-trigger"
              onPointerDown={(event) => event.preventDefault()}
              onClick={() => void translateSelection()}
            >
              <Languages size={16} />
              {targetLanguage === 'en' ? '译成英文' : '译成中文'}
            </button>
          ) : (
            <>
              <div className="selection-translation-header">
                <span><Languages size={15} /> 划词翻译</span>
                <button type="button" onClick={close} title="关闭" aria-label="关闭划词翻译">
                  <X size={15} />
                </button>
              </div>
              {selection.phase === 'loading' && (
                <div className="selection-translation-loading">
                  <LoaderCircle size={18} />
                  <span>翻译中</span>
                </div>
              )}
              {selection.phase === 'done' && (
                <p className="selection-translation-result">{selection.translation}</p>
              )}
              {selection.phase === 'error' && (
                <div className="selection-translation-error" role="alert">
                  <p>{selection.error}</p>
                  {selection.characterCount <= MAX_SELECTION_CHARS && (
                    <button type="button" onClick={() => void translateSelection()}>
                      <RefreshCw size={14} /> 重试
                    </button>
                  )}
                </div>
              )}
            </>
          )}
        </div>,
        document.body
      )}
    </>
  )
}
