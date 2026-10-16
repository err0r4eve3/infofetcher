export type Capture = {
  id: string; article_id: string; captured_at: string; sha256: string; byte_length: number
  details: { article_title?: string; final_url?: string; http_status?: number }
}
export type Material = {
  id: string; title: string; url: string; dependency_group: number; content: string
  text_sha256: string; full_text_sha256: string; content_truncated: boolean
  document?: { id: string; sha256: string; extraction_sha256: string }
  retrieval?: { requested_url: string; final_url: string; response_sha256: string }
  reported_published_at: string | null; retrieved_at: string
}
export type ResearchReport = {
  run_id: string; case_id: string; event_id: string; event_title: string
  generated_at: string; query: string; status: 'analyzed' | 'insufficient'
  total_sources: number; dependency_groups: number; captures: Capture[]
  steps: { tool: string; status: string; message: string }[]
  leads: { article_id: string; title: string; url: string }[]
  materials: Material[]
  fragments: { id?: string; evidence_ids?: string[]; kind: string; excerpt: string; numbers: string[]; article_ids: string[]; dependency_groups: number }[]
  findings: { kind: string; message: string; article_ids: string[] }[]
  warnings: string[]
  mode?: 'online' | 'local'
  evidence?: { id: string; article_id: string; source_url: string; source_title: string; source_kind: string; start_byte: number; end_byte: number; exact_excerpt: string }[]
  citation_audit?: { status: string; checked: number; semantic_support: string }
  research?: {
    provider: string; stop_reason: string
    gaps: { kind: string; message: string }[]
    budget: { searches_used: number; searches_limit: number; pages_attempted: number; pages_limit: number; pages_read: number; seconds_limit: number }
    rounds: { query: string; reason: string; status: string; candidates_added: number; pages_added: number }[]
    candidates: { url: string; title: string; status: string; message: string; material_id: string | null; evidence_ids: string[] }[]
  }
}
export type Review = {
  id: number; target_kind: 'finding' | 'fragment'; target_index: number
  verdict: 'confirmed' | 'rejected' | 'needs_review'; note: string; actor: string; created_at: string
}
export type Entity = { id: string; kind: 'person' | 'organization' | 'location'; name: string; aliases: string[] }
export type Mention = {
  id: string; run_id: string; entity_id: string; article_id: string; matched_alias: string; excerpt: string
  status: 'suggested' | 'confirmed' | 'rejected'; entity: Entity; case_id?: string; case_title?: string
}
export type Relation = {
  id: string; subject_id: string; object_id: string; subject_name: string; object_name: string
  predicate: string; article_id: string; excerpt: string; status: 'confirmed' | 'rejected'
}
export type RunBundle = {
  id: string; case_id: string; report: ResearchReport; report_sha256: string; created_by: string
  reviews: Review[]; mentions: Mention[]; relations: Relation[]
  entity_audit: { id: number; actor: string; action: string; details: Record<string, unknown>; created_at: string }[]
}
export type CaseSummary = {
  id: string; event_id: string; title: string; event_available: boolean; updated_at: string; origin?: 'event' | 'topic'
  run_count?: number; capture_count?: number; document_count?: number
}
export type CaseDetail = {
  case: CaseSummary; captures: Capture[]; next_offset: number | null
  runs: { id: string; created_at: string; created_by: string; query: string; status: string; material_count: number }[]
}
export function safeLink(value: string): string | undefined {
  if (/^\/api\/v2\/osint\/documents\/[a-f0-9-]{36}\/original$/.test(value)) return value
  try {
    const url = new URL(value)
    return ['http:', 'https:'].includes(url.protocol) && !url.username && !url.password ? url.href : undefined
  } catch { return undefined }
}
export function caseLink(id: string, runId?: string) {
  return `/?radar=investigations&case=${encodeURIComponent(id)}${runId ? `&run=${encodeURIComponent(runId)}` : ''}`
}
