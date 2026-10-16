import assert from 'node:assert/strict'
import test from 'node:test'
import { watchDisplayName } from '../src/api.ts'

test('tracked-event watch names hide the uniqueness suffix', () => {
  assert.equal(watchDisplayName('追踪 · OpenAI agent infiltrated site · ad0377'), '追踪 · OpenAI agent infiltrated site')
  assert.equal(watchDisplayName('AI Agent 生产化'), 'AI Agent 生产化')
  assert.equal(watchDisplayName('版本 · v1.2.3'), '版本 · v1.2.3')
})
