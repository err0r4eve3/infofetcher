import assert from 'node:assert/strict'
import test from 'node:test'
import { coverageLabel, evidenceForDisplay, evidenceTimestamp, matchesCoverage, sourceNames } from '../src/coverageModel.ts'

test('coverage uses source families, not repeated articles or feed titles', () => {
  const repeated = { independent_source_count: 1, source_count: 9, source_titles: ['A', 'A mirror'] }
  assert.equal(matchesCoverage(repeated, 'single'), true)
  assert.equal(matchesCoverage(repeated, 'multiple'), false)
  assert.equal(matchesCoverage({ independent_source_count: 2 }, 'multiple'), true)
})

test('missing coverage is unknown, never single-source', () => {
  for (const count of [0, -1, NaN]) {
    assert.equal(matchesCoverage({ independent_source_count: count }, 'all'), true)
    assert.equal(matchesCoverage({ independent_source_count: count }, 'single'), false)
    assert.equal(matchesCoverage({ independent_source_count: count }, 'multiple'), false)
    assert.equal(coverageLabel(count), '来源待确认')
  }
})

test('feed labels are trimmed and deduplicated without inventing publisher identity', () => {
  assert.deepEqual(sourceNames([' BBC ', 'BBC', '', 'BBC World', 'Reuters']), ['BBC', 'BBC World', 'Reuters'])
  assert.deepEqual(sourceNames(undefined), [])
})

const evidence = [
  {article_id:'a',source_title:' Reuters ',title:'A',url:'https://example.org/a',excerpt:'',quality_score:95,published_at:'2026-09-26T12:00:00Z'},
  {article_id:'b',source_title:'BBC',title:'B',url:'https://example.org/b',excerpt:'',quality_score:80,published_at:'2026-09-27T12:00:00Z'},
  {article_id:'c',source_title:'BBC',title:'C',url:'https://example.org/c',excerpt:'',quality_score:80,published_at:'unknown'}
]

test('sorting and filtering do not mutate API evidence; undated evidence sorts last', () => {
  assert.deepEqual(evidenceForDisplay(evidence, '', 'latest').map(e=>e.article_id), ['b', 'a', 'c'])
  assert.deepEqual(evidenceForDisplay(evidence, '', 'quality').map(e=>e.article_id), ['a', 'b', 'c'])
  assert.deepEqual(evidenceForDisplay(evidence, 'Reuters', 'latest').map(e=>e.article_id), ['a'])
  assert.deepEqual(evidenceForDisplay(evidence, 'BBC', 'latest').map(e=>e.article_id), ['b', 'c'])
  assert.deepEqual(evidence.map(e=>e.article_id), ['a', 'b', 'c'])
  assert.deepEqual(evidenceForDisplay([], '', 'latest'), [])
})

test('publication time stays unknown instead of inheriting the API sorting fallback', () => {
  const undated = { ...evidence[0], reported_published_at: null, fetched_at: '2026-09-28T12:00:00Z' }
  assert.deepEqual(evidenceTimestamp(undated), { kind: 'collected', value: undated.fetched_at })
  assert.deepEqual(evidenceTimestamp({ ...undated, reported_published_at: evidence[0].published_at }), {
    kind: 'published', value: evidence[0].published_at,
  })
  assert.deepEqual(evidenceTimestamp(evidence[0]), { kind: 'legacy', value: evidence[0].published_at })
  assert.deepEqual(evidenceForDisplay([evidence[1], undated], '', 'latest').map(e => e.article_id), ['a', 'b'])
})
