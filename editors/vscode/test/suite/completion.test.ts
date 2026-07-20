import * as assert from 'assert';
import { getCompletions, getKeywordDocs } from '../../src/lsp/completion';

suite('Completion Test Suite', () => {

  test('test_completion_empty_prefix', () => {
    const items = getCompletions('');
    assert.ok(items.length >= 18);
  });

  test('test_completion_prefix_f', () => {
    const items = getCompletions('f');
    const labels = items.map((i) => i.label);
    assert.ok(labels.includes('fn'));
    assert.ok(labels.includes('for'));
    assert.ok(labels.includes('false'));
  });

  test('test_completion_prefix_le', () => {
    const items = getCompletions('le');
    const labels = items.map((i) => i.label);
    assert.ok(labels.includes('let'));
  });

  test('test_completion_no_match', () => {
    const items = getCompletions('zzz');
    assert.strictEqual(items.length, 0);
  });

  test('test_keyword_docs_fn', () => {
    const docs = getKeywordDocs('fn');
    assert.ok(docs !== null);
    assert.ok(docs!.includes('fn'));
  });

  test('test_keyword_docs_unknown', () => {
    const docs = getKeywordDocs('notakeyword');
    assert.strictEqual(docs, null);
  });
});
