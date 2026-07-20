import * as assert from 'assert';
import { parseCompilerErrors } from '../../src/lsp/diagnostics';
import { TextDocument } from 'vscode-languageserver-textdocument';

function makeDoc(text: string): TextDocument {
  return TextDocument.create('file:///test.nuzo', 'nuzo', 1, text);
}

suite('Diagnostics Test Suite', () => {

  test('test_parse_error_format1', () => {
    const doc = makeDoc('let x = ;\n');
    const diags = parseCompilerErrors('error: unexpected token at line 1:col 10', doc);
    assert.strictEqual(diags.length, 1);
    assert.strictEqual(diags[0].range.start.line, 0);
  });

  test('test_parse_error_format2', () => {
    const doc = makeDoc('let x = 1;\nlet y = ;\n');
    const diags = parseCompilerErrors('error: unexpected token (line 2)', doc);
    assert.strictEqual(diags.length, 1);
    assert.strictEqual(diags[0].range.start.line, 1);
  });

  test('test_parse_error_format3', () => {
    const doc = makeDoc('fn main() {\n  invalid\n}\n');
    const diags = parseCompilerErrors('test.nuzo:2:3: error: unknown identifier', doc);
    assert.strictEqual(diags.length, 1);
    assert.strictEqual(diags[0].range.start.line, 1);
    assert.strictEqual(diags[0].range.start.character, 2);
  });

  test('test_parse_no_errors', () => {
    const doc = makeDoc('let x = 1;\n');
    const diags = parseCompilerErrors('', doc);
    assert.strictEqual(diags.length, 0);
  });

  test('test_parse_fallback_raw_stderr', () => {
    const doc = makeDoc('code\n');
    const diags = parseCompilerErrors('some unstructured error output', doc);
    assert.strictEqual(diags.length, 1);
    assert.ok(diags[0].message.includes('unstructured'));
  });
});
