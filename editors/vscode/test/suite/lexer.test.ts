import * as assert from 'assert';
import { tokenize, TokenKind } from '../../src/lsp/lexer';

suite('Lexer Test Suite', () => {

  test('test_keyword_all', () => {
    const src = 'fn let const struct enum trait impl if else while for match return break continue true false nil';
    const tokens = tokenize(src).filter((t) => t.kind !== TokenKind.Whitespace && t.kind !== TokenKind.Eof);
    assert.strictEqual(tokens.length, 18);
    for (const tok of tokens) {
      assert.strictEqual(tok.kind, TokenKind.Keyword, `Expected keyword, got ${tok.kind} for "${tok.text}"`);
    }
  });

  test('test_number_decimal', () => {
    const tokens = tokenize('123').filter((t) => t.kind !== TokenKind.Eof);
    assert.strictEqual(tokens[0].kind, TokenKind.Number);
    assert.strictEqual(tokens[0].text, '123');
  });

  test('test_number_hex', () => {
    const tokens = tokenize('0xFF').filter((t) => t.kind !== TokenKind.Eof);
    assert.strictEqual(tokens[0].kind, TokenKind.Number);
    assert.strictEqual(tokens[0].text, '0xFF');
  });

  test('test_number_octal', () => {
    const tokens = tokenize('0o77').filter((t) => t.kind !== TokenKind.Eof);
    assert.strictEqual(tokens[0].kind, TokenKind.Number);
    assert.strictEqual(tokens[0].text, '0o77');
  });

  test('test_number_binary', () => {
    const tokens = tokenize('0b1010').filter((t) => t.kind !== TokenKind.Eof);
    assert.strictEqual(tokens[0].kind, TokenKind.Number);
    assert.strictEqual(tokens[0].text, '0b1010');
  });

  test('test_number_float', () => {
    const tokens = tokenize('3.14').filter((t) => t.kind !== TokenKind.Eof);
    assert.strictEqual(tokens[0].kind, TokenKind.Number);
    assert.strictEqual(tokens[0].text, '3.14');
  });

  test('test_number_zero', () => {
    const tokens = tokenize('0').filter((t) => t.kind !== TokenKind.Eof);
    assert.strictEqual(tokens[0].kind, TokenKind.Number);
    assert.strictEqual(tokens[0].text, '0');
  });

  test('test_number_scientific', () => {
    const tokens = tokenize('1e10').filter((t) => t.kind !== TokenKind.Eof);
    assert.strictEqual(tokens[0].kind, TokenKind.Number);
    assert.strictEqual(tokens[0].text, '1e10');
  });

  test('test_string_double_quote', () => {
    const tokens = tokenize('"hello"').filter((t) => t.kind !== TokenKind.Eof);
    assert.strictEqual(tokens[0].kind, TokenKind.String);
    assert.strictEqual(tokens[0].text, '"hello"');
  });

  test('test_string_escape', () => {
    const tokens = tokenize('"a\\nb"').filter((t) => t.kind !== TokenKind.Eof);
    assert.strictEqual(tokens[0].kind, TokenKind.String);
    assert.strictEqual(tokens[0].text, '"a\\nb"');
  });

  test('test_string_empty', () => {
    const tokens = tokenize('""').filter((t) => t.kind !== TokenKind.Eof);
    assert.strictEqual(tokens[0].kind, TokenKind.String);
    assert.strictEqual(tokens[0].text, '""');
  });

  test('test_char_literal', () => {
    const tokens = tokenize("'a'").filter((t) => t.kind !== TokenKind.Eof);
    assert.strictEqual(tokens[0].kind, TokenKind.String);
    assert.strictEqual(tokens[0].text, "'a'");
  });

  test('test_comment_single', () => {
    const tokens = tokenize('// comment').filter((t) => t.kind !== TokenKind.Eof);
    assert.strictEqual(tokens[0].kind, TokenKind.Comment);
    assert.strictEqual(tokens[0].text, '// comment');
  });

  test('test_comment_multi', () => {
    const tokens = tokenize('/* a */').filter((t) => t.kind !== TokenKind.Eof);
    assert.strictEqual(tokens[0].kind, TokenKind.Comment);
    assert.strictEqual(tokens[0].text, '/* a */');
  });

  test('test_comment_doc', () => {
    const tokens = tokenize('/// doc').filter((t) => t.kind !== TokenKind.Eof);
    assert.strictEqual(tokens[0].kind, TokenKind.Comment);
    assert.strictEqual(tokens[0].text, '/// doc');
  });

  test('test_operator_multi_char', () => {
    const src = '== != <= >= && || -> => += -=';
    const tokens = tokenize(src).filter((t) => t.kind === TokenKind.Operator);
    assert.strictEqual(tokens.length, 10);
    assert.strictEqual(tokens[0].text, '==');
    assert.strictEqual(tokens[2].text, '<=');
    assert.strictEqual(tokens[6].text, '->');
    assert.strictEqual(tokens[7].text, '=>');
  });

  test('test_punctuation', () => {
    const src = '. , ; : ( ) [ ] { }';
    const tokens = tokenize(src).filter((t) => t.kind === TokenKind.Punctuation);
    assert.strictEqual(tokens.length, 10);
  });

  test('test_identifier_simple', () => {
    const tokens = tokenize('foo').filter((t) => t.kind !== TokenKind.Eof);
    assert.strictEqual(tokens[0].kind, TokenKind.Identifier);
    assert.strictEqual(tokens[0].text, 'foo');
  });

  test('test_identifier_underscore', () => {
    const tokens = tokenize('_foo').filter((t) => t.kind !== TokenKind.Eof);
    assert.strictEqual(tokens[0].kind, TokenKind.Identifier);
    assert.strictEqual(tokens[0].text, '_foo');
  });

  test('test_empty_source', () => {
    const tokens = tokenize('');
    assert.strictEqual(tokens.length, 1);
    assert.strictEqual(tokens[0].kind, TokenKind.Eof);
  });

  test('test_full_program', () => {
    const src = `fn main() {
  let x = 42;
  let s = "hello";
  // comment
  print(x);
}`;
    const tokens = tokenize(src);
    const keywords = tokens.filter((t) => t.kind === TokenKind.Keyword);
    assert.ok(keywords.some((k) => k.text === 'fn'));
    assert.ok(keywords.some((k) => k.text === 'let'));
    const numbers = tokens.filter((t) => t.kind === TokenKind.Number);
    assert.ok(numbers.some((n) => n.text === '42'));
    const strings = tokens.filter((t) => t.kind === TokenKind.String);
    assert.ok(strings.some((s) => s.text === '"hello"'));
  });
});
