const BLOCK_END_TAG = /<\/(?:article|blockquote|div|h[1-6]|li|p|pre|section)>/gi
const BLOCK_START_TAG = /<(?:article|blockquote|div|h[1-6]|li|p|pre|section)\b[^>]*>/gi
const INLINE_TAG_BOUNDARY = /<\/(?:a|abbr|b|code|em|i|mark|s|small|span|strong|sub|sup|time|u)>\s*<(?:a|abbr|b|code|em|i|mark|s|small|span|strong|sub|sup|time|u)\b[^>]*>/gi

function normalizeParagraph(value: string): string {
  return value
    .replace(/[\t ]*\n[\t ]*/g, ' ')
    .replace(/[\t ]{2,}/g, ' ')
    .trim()
}

function stripArticleMarkup(value: string): string {
  return value
    .replace(/<(?:script|style)\b[^>]*>[\s\S]*?<\/(?:script|style)>/gi, '')
    .replace(/<\s*br\s*\/?\s*>/gi, '\n\n')
    .replace(BLOCK_END_TAG, '\n\n')
    .replace(BLOCK_START_TAG, '')
    .replace(INLINE_TAG_BOUNDARY, ' ')
    .replace(/<[^>]*>/g, '')
    .replace(/&(nbsp|#160);/gi, ' ')
    .replace(/&(amp|#38);/gi, '&')
    .replace(/&(lt|#60);/gi, '<')
    .replace(/&(gt|#62);/gi, '>')
    .replace(/&(quot|#34);/gi, '"')
    .replace(/&#39;/g, "'")
    .replace(/\r\n?/g, '\n')
    .replace(/\u00a0/g, ' ')
}

function splitSentenceParagraphs(value: string): string[] {
  const separated = value
    .replace(/([。！？])(?=\s*[\u3400-\u9fffA-Z0-9“‘"（(])/g, '$1\n\n')
    .replace(/([.!?][”’"]?)(?=\s*(?:[A-Z“‘"(\[]))/g, '$1\n\n')

  const paragraphs = separated
    .split(/\n{2,}/)
    .map(normalizeParagraph)
    .filter(Boolean)

  return paragraphs.length > 1 ? paragraphs : [value]
}

export function formatArticleParagraphs(content: string): string[] {
  const explicitParagraphs = stripArticleMarkup(content)
    .split(/\n{2,}/)
    .map(normalizeParagraph)
    .filter(Boolean)

  if (explicitParagraphs.length !== 1) return explicitParagraphs
  return explicitParagraphs.length === 1 ? splitSentenceParagraphs(explicitParagraphs[0]) : []
}
