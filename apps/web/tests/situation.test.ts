import assert from 'node:assert/strict'
import test from 'node:test'
import type { EventBriefItem } from '../src/api.ts'
import { isCorrection, readHiddenPanels, searchSituation, situationPanelItems } from '../src/situation.ts'

function event(id: string, extra: Partial<EventBriefItem> = {}): EventBriefItem {
  return {
    event_id: id, title: '模型发布', what_happened: '开源模型升级', report_section: 'technology',
    revision_type: 'ADD', why_it_matters: '', why_now: '', confidence_label: '', confidence_score: 0,
    source_quality_score: 0, impact_score: 0, novelty_score: 0, urgency_score: 0, relevance_score: 0,
    total_score: 10, source_count: 1, independent_source_count: 1, source_titles: ['Example News'],
    watch_ids: [], watch_names: [], representative_article_id: id, published_at: '2026-09-28T00:00:00Z',
    update_count: 0, has_grounded_summary: false, feedback: null, ...extra
  }
}

test('search combines title, summary, sources and watch terms without regex interpretation', () => {
  const items = [event('a', { watch_names: ['AI 研究'] }), event('b', { title: 'C++ 更新', what_happened: '编译器改进' })]
  assert.deepEqual(searchSituation(items, '  MODEL '), [])
  assert.deepEqual(searchSituation(items, '模型 EXAMPLE 研究').map((x) => x.event_id), ['a'])
  assert.deepEqual(searchSituation(items, 'C++').map((x) => x.event_id), ['b'])
  assert.deepEqual(searchSituation(items, '   '), items)
})

test('priority and publication order differ; sorting never mutates the report', () => {
  const items = [event('recent', { published_at: '2026-09-28T02:00:00Z' }), event('important', { total_score: 90 }), event('unknown', { published_at: '' })]
  assert.deepEqual(situationPanelItems(items, 'priority').map((x) => x.event_id), ['important', 'recent', 'unknown'])
  assert.deepEqual(situationPanelItems(items, 'recent').map((x) => x.event_id), ['recent', 'important', 'unknown'])
  assert.deepEqual(items.map((x) => x.event_id), ['recent', 'important', 'unknown'])
})

test('only correction/disagreement revisions and actual watch IDs enter specialist panels', () => {
  const items = [event('correction', { revision_type: 'CORRECT' }), event('disagreement', { revision_type: 'CONTRADICT' }), event('confirmation', { revision_type: 'CONFIRM', watch_ids: ['watch'] }), event('name-only', { watch_names: ['not a match'] })]
  assert.equal(items.filter(isCorrection).length, 2)
  assert.deepEqual(situationPanelItems(items, 'corrections').map((x) => x.event_id), ['correction', 'disagreement'])
  assert.deepEqual(situationPanelItems(items, 'watches').map((x) => x.event_id), ['confirmation'])
  assert.deepEqual(situationPanelItems([], 'priority'), [])
})

test('stored panel preferences tolerate corruption, unknown panels and duplicates', () => {
  assert.deepEqual(readHiddenPanels('{broken'), [])
  assert.deepEqual(readHiddenPanels('{"priority":false}'), [])
  assert.deepEqual(readHiddenPanels('["recent","recent","unknown",null]'), ['recent'])
  assert.deepEqual(readHiddenPanels(null), [])
})
