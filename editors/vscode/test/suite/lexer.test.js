"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();
Object.defineProperty(exports, "__esModule", { value: true });
const assert = __importStar(require("assert"));
const lexer_1 = require("../../src/lsp/lexer");
suite('Lexer Test Suite', () => {
    test('test_keyword_all', () => {
        const src = 'fn let const struct enum trait impl if else while for match return break continue true false nil';
        const tokens = (0, lexer_1.tokenize)(src).filter((t) => t.kind !== lexer_1.TokenKind.Whitespace && t.kind !== lexer_1.TokenKind.Eof);
        assert.strictEqual(tokens.length, 18);
        for (const tok of tokens) {
            assert.strictEqual(tok.kind, lexer_1.TokenKind.Keyword, `Expected keyword, got ${tok.kind} for "${tok.text}"`);
        }
    });
    test('test_number_decimal', () => {
        const tokens = (0, lexer_1.tokenize)('123').filter((t) => t.kind !== lexer_1.TokenKind.Eof);
        assert.strictEqual(tokens[0].kind, lexer_1.TokenKind.Number);
        assert.strictEqual(tokens[0].text, '123');
    });
    test('test_number_hex', () => {
        const tokens = (0, lexer_1.tokenize)('0xFF').filter((t) => t.kind !== lexer_1.TokenKind.Eof);
        assert.strictEqual(tokens[0].kind, lexer_1.TokenKind.Number);
        assert.strictEqual(tokens[0].text, '0xFF');
    });
    test('test_number_octal', () => {
        const tokens = (0, lexer_1.tokenize)('0o77').filter((t) => t.kind !== lexer_1.TokenKind.Eof);
        assert.strictEqual(tokens[0].kind, lexer_1.TokenKind.Number);
        assert.strictEqual(tokens[0].text, '0o77');
    });
    test('test_number_binary', () => {
        const tokens = (0, lexer_1.tokenize)('0b1010').filter((t) => t.kind !== lexer_1.TokenKind.Eof);
        assert.strictEqual(tokens[0].kind, lexer_1.TokenKind.Number);
        assert.strictEqual(tokens[0].text, '0b1010');
    });
    test('test_number_float', () => {
        const tokens = (0, lexer_1.tokenize)('3.14').filter((t) => t.kind !== lexer_1.TokenKind.Eof);
        assert.strictEqual(tokens[0].kind, lexer_1.TokenKind.Number);
        assert.strictEqual(tokens[0].text, '3.14');
    });
    test('test_number_zero', () => {
        const tokens = (0, lexer_1.tokenize)('0').filter((t) => t.kind !== lexer_1.TokenKind.Eof);
        assert.strictEqual(tokens[0].kind, lexer_1.TokenKind.Number);
        assert.strictEqual(tokens[0].text, '0');
    });
    test('test_number_scientific', () => {
        const tokens = (0, lexer_1.tokenize)('1e10').filter((t) => t.kind !== lexer_1.TokenKind.Eof);
        assert.strictEqual(tokens[0].kind, lexer_1.TokenKind.Number);
        assert.strictEqual(tokens[0].text, '1e10');
    });
    test('test_string_double_quote', () => {
        const tokens = (0, lexer_1.tokenize)('"hello"').filter((t) => t.kind !== lexer_1.TokenKind.Eof);
        assert.strictEqual(tokens[0].kind, lexer_1.TokenKind.String);
        assert.strictEqual(tokens[0].text, '"hello"');
    });
    test('test_string_escape', () => {
        const tokens = (0, lexer_1.tokenize)('"a\\nb"').filter((t) => t.kind !== lexer_1.TokenKind.Eof);
        assert.strictEqual(tokens[0].kind, lexer_1.TokenKind.String);
        assert.strictEqual(tokens[0].text, '"a\\nb"');
    });
    test('test_string_empty', () => {
        const tokens = (0, lexer_1.tokenize)('""').filter((t) => t.kind !== lexer_1.TokenKind.Eof);
        assert.strictEqual(tokens[0].kind, lexer_1.TokenKind.String);
        assert.strictEqual(tokens[0].text, '""');
    });
    test('test_char_literal', () => {
        const tokens = (0, lexer_1.tokenize)("'a'").filter((t) => t.kind !== lexer_1.TokenKind.Eof);
        assert.strictEqual(tokens[0].kind, lexer_1.TokenKind.String);
        assert.strictEqual(tokens[0].text, "'a'");
    });
    test('test_comment_single', () => {
        const tokens = (0, lexer_1.tokenize)('// comment').filter((t) => t.kind !== lexer_1.TokenKind.Eof);
        assert.strictEqual(tokens[0].kind, lexer_1.TokenKind.Comment);
        assert.strictEqual(tokens[0].text, '// comment');
    });
    test('test_comment_multi', () => {
        const tokens = (0, lexer_1.tokenize)('/* a */').filter((t) => t.kind !== lexer_1.TokenKind.Eof);
        assert.strictEqual(tokens[0].kind, lexer_1.TokenKind.Comment);
        assert.strictEqual(tokens[0].text, '/* a */');
    });
    test('test_comment_doc', () => {
        const tokens = (0, lexer_1.tokenize)('/// doc').filter((t) => t.kind !== lexer_1.TokenKind.Eof);
        assert.strictEqual(tokens[0].kind, lexer_1.TokenKind.Comment);
        assert.strictEqual(tokens[0].text, '/// doc');
    });
    test('test_operator_multi_char', () => {
        const src = '== != <= >= && || -> => += -=';
        const tokens = (0, lexer_1.tokenize)(src).filter((t) => t.kind === lexer_1.TokenKind.Operator);
        assert.strictEqual(tokens.length, 10);
        assert.strictEqual(tokens[0].text, '==');
        assert.strictEqual(tokens[2].text, '<=');
        assert.strictEqual(tokens[6].text, '->');
        assert.strictEqual(tokens[7].text, '=>');
    });
    test('test_punctuation', () => {
        const src = '. , ; : ( ) [ ] { }';
        const tokens = (0, lexer_1.tokenize)(src).filter((t) => t.kind === lexer_1.TokenKind.Punctuation);
        assert.strictEqual(tokens.length, 10);
    });
    test('test_identifier_simple', () => {
        const tokens = (0, lexer_1.tokenize)('foo').filter((t) => t.kind !== lexer_1.TokenKind.Eof);
        assert.strictEqual(tokens[0].kind, lexer_1.TokenKind.Identifier);
        assert.strictEqual(tokens[0].text, 'foo');
    });
    test('test_identifier_underscore', () => {
        const tokens = (0, lexer_1.tokenize)('_foo').filter((t) => t.kind !== lexer_1.TokenKind.Eof);
        assert.strictEqual(tokens[0].kind, lexer_1.TokenKind.Identifier);
        assert.strictEqual(tokens[0].text, '_foo');
    });
    test('test_empty_source', () => {
        const tokens = (0, lexer_1.tokenize)('');
        assert.strictEqual(tokens.length, 1);
        assert.strictEqual(tokens[0].kind, lexer_1.TokenKind.Eof);
    });
    test('test_full_program', () => {
        const src = `fn main() {
  let x = 42;
  let s = "hello";
  // comment
  print(x);
}`;
        const tokens = (0, lexer_1.tokenize)(src);
        const keywords = tokens.filter((t) => t.kind === lexer_1.TokenKind.Keyword);
        assert.ok(keywords.some((k) => k.text === 'fn'));
        assert.ok(keywords.some((k) => k.text === 'let'));
        const numbers = tokens.filter((t) => t.kind === lexer_1.TokenKind.Number);
        assert.ok(numbers.some((n) => n.text === '42'));
        const strings = tokens.filter((t) => t.kind === lexer_1.TokenKind.String);
        assert.ok(strings.some((s) => s.text === '"hello"'));
    });
});
//# sourceMappingURL=lexer.test.js.map