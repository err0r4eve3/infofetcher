import assert from 'node:assert/strict'
import test from 'node:test'
import type { Feed, EventBriefItem } from '../src/api.ts'
import { filterSources } from '../src/sourceFilters.ts'
import { EMPTY_REPORT_FILTERS, filterReportItems } from '../src/reportFilters.ts'

const items = [
  { event_id: 'a', title: 'C++ 安全更新', what_happened: '修复漏洞', source_titles: [' CISA '], watch_names: ['工程'], has_grounded_summary: true },
  { event_id: 'b', title: 'C++ 课程促销', what_happened: '培训', source_titles: ['CISA Copy'], watch_names: [], has_grounded_summary: false },
  { event_id: 'c', title: '火箭发射', what_happened: '任务完成', source_titles: ['NASA'], watch_names: [], has_grounded_summary: false },
] as EventBriefItem[]

test('report filters intersect literal inclusion, any exclusion, exact source and evidence', () => {
  assert.deepEqual(filterReportItems(items, { ...EMPTY_REPORT_FILTERS, query: ' c++ 工程 ', exclude: '促销 培训', source: 'CISA', evidence: 'grounded' }).map(x => x.event_id), ['a'])
  assert.deepEqual(filterReportItems(items, { ...EMPTY_REPORT_FILTERS, source: 'CISA Copy', evidence: 'grounded' }), [])
  assert.deepEqual(filterReportItems(items, { ...EMPTY_REPORT_FILTERS, evidence: 'unprocessed' }).map(x => x.event_id), ['b', 'c'])
  assert.deepEqual(filterReportItems(items, { ...EMPTY_REPORT_FILTERS, source: 'missing' }), [])
  assert.deepEqual(filterReportItems(items, EMPTY_REPORT_FILTERS), items)
})

test('negative terms do not become regular expressions and source names are not partial matches', () => {
  assert.deepEqual(filterReportItems(items, { ...EMPTY_REPORT_FILTERS, exclude: 'C++' }).map(x => x.event_id), ['c'])
  assert.deepEqual(filterReportItems(items, { ...EMPTY_REPORT_FILTERS, source: 'CIS' }), [])
  assert.deepEqual(filterReportItems(items, { ...EMPTY_REPORT_FILTERS, query: '[' }), [])
})

const feeds = [
  { id: 'a', title: 'NASA', feed_url: 'https://nasa.gov/feed', last_success_at: '2026-09-30', consecutive_failures: 0, last_error: null, muted: false },
  { id: 'b', title: 'Security', feed_url: 'https://example.org/feed', last_success_at: '2026-09-29', consecutive_failures: 2, last_error: 'timeout', muted: true },
  { id: 'c', title: 'New source', feed_url: 'https://example.net/feed', last_success_at: null, consecutive_failures: 0, last_error: null, muted: false },
] as Feed[]

test('collection state stays separate from mute and missing success is not healthy', () => {
  assert.deepEqual(filterSources(feeds, '', 'unverified').map(x => x.id), ['c'])
  assert.deepEqual(filterSources(feeds, 'EXAMPLE Security', 'failing').map(x => x.id), ['b'])
  assert.deepEqual(filterSources(feeds, '', 'muted').map(x => x.id), ['b'])
  assert.deepEqual(filterSources(feeds, '', 'active').map(x => x.id), ['a', 'c'])
  assert.deepEqual(filterSources(feeds, 'NASA', 'failing'), [])
  assert.deepEqual(filterSources(feeds, '', 'all'), feeds)
})
