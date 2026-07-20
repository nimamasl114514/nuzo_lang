// ============================================================
// Nuzo Language Support for CodeMirror 6
//
// Uses StreamLanguage (CM5-style stream parser) for simplicity.
// No Lezer toolchain required (per T9 spec — 方案 B).
//
// Keyword sources:
//   - English: crates/nuzo-frontend/src/token.rs L407-430
//   - Chinese aliases: same source (如果/否则/当/...)
// ============================================================

import { StreamLanguage, LanguageSupport } from '@codemirror/language';

// ---------- English keywords ----------
const KEYWORDS = new Set([
  // Control flow
  'if', 'else', 'while', 'for', 'in', 'loop', 'break', 'continue',
  'match', 'return', 'try', 'catch', 'out', 'keep',
  // Declaration
  'fn',
  // Module system
  'import', 'as', 'lazy',
  // Logical operators (word form)
  'and', 'or', 'not',
  // Boolean / null literals
  'true', 'false', 'nil',
]);

// ---------- Chinese keyword aliases ----------
// Nuzo supports bilingual keywords (token.rs L407-430).
const CN_KEYWORDS = new Set([
  '如果', '否则', '当', '遍历', '在', '循环', '跳出', '继续',
  '匹配', '返回', '尝试', '捕获', '抛出', '始终',
  '函数',
  '导入', '作为', '懒',
  '并且', '或者', '非',
  '真', '假', '空',
]);

// ---------- Builtin functions ----------
// Common stdlib / helper functions exposed as builtins.
const BUILTINS = new Set([
  'print', 'println',
  'len', 'push', 'pop', 'concat', 'split', 'join',
  'map', 'filter', 'reduce', 'sort', 'reverse',
  'keys', 'values', 'to_string', 'type_of', 'assert',
]);

// ---------- Stream parser definition ----------
const nuzoStreamParser = StreamLanguage.define({
  name: 'nuzo',
  startState() {
    return { inBlockComment: false };
  },
  token(stream, state) {
    // ----- Block comment (continuation across lines) -----
    if (state.inBlockComment) {
      if (stream.skipTo('*/')) {
        stream.next(2);
        state.inBlockComment = false;
      } else {
        stream.skipToEnd();
      }
      return 'comment';
    }

    // ----- Doc comment: /// (triple-slash, line) -----
    if (stream.match('///')) {
      stream.skipToEnd();
      return 'comment';
    }

    // ----- Line comment: // -----
    if (stream.match('//')) {
      stream.skipToEnd();
      return 'comment';
    }

    // ----- Block comment start: /* ... */ -----
    if (stream.match('/*')) {
      state.inBlockComment = true;
      if (stream.skipTo('*/')) {
        stream.next(2);
        state.inBlockComment = false;
      } else {
        stream.skipToEnd();
      }
      return 'comment';
    }

    // ----- Double-quoted string -----
    if (stream.match('"')) {
      while (!stream.eol()) {
        const ch = stream.next();
        if (ch === '\\') {
          if (!stream.eol()) stream.next();
        } else if (ch === '"') {
          break;
        }
      }
      return 'string';
    }

    // ----- Backtick interpolated string -----
    if (stream.match('`')) {
      while (!stream.eol()) {
        const ch = stream.next();
        if (ch === '\\') {
          if (!stream.eol()) stream.next();
        } else if (ch === '`') {
          break;
        }
      }
      return 'string';
    }

    // ----- Single-quoted string / char -----
    if (stream.match("'")) {
      while (!stream.eol()) {
        const ch = stream.next();
        if (ch === '\\') {
          if (!stream.eol()) stream.next();
        } else if (ch === "'") {
          break;
        }
      }
      return 'string';
    }

    // ----- Numbers (hex / octal / binary / float / int) -----
    if (stream.match(/\b0[xX][0-9a-fA-F]+\b/)) return 'number';
    if (stream.match(/\b0[oO][0-7]+\b/)) return 'number';
    if (stream.match(/\b0[bB][01]+\b/)) return 'number';
    if (stream.match(/\b[0-9]+\.[0-9]+([eE][+-]?[0-9]+)?\b/)) return 'number';
    if (stream.match(/\b[0-9]+[eE][+-]?[0-9]+\b/)) return 'number';
    if (stream.match(/\b[0-9]+\b/)) return 'number';

    // ----- Identifiers (English + Chinese Han) -----
    // \u4e00-\u9fff covers CJK Unified Ideographs (common Chinese)
    if (stream.match(/[A-Za-z_\u4e00-\u9fff][A-Za-z0-9_\u4e00-\u9fff]*/)) {
      const word = stream.current();
      if (KEYWORDS.has(word) || CN_KEYWORDS.has(word)) return 'keyword';
      if (BUILTINS.has(word)) return 'builtin';
      // Type names: capitalized identifiers (per tmLanguage entity.name.type)
      if (/^[A-Z]/.test(word)) return 'typeName';
      return 'variable';
    }

    // ----- Operators (multi-char first, then single) -----
    if (stream.match(/(==|!=|<=|>=|->|=>|\+=|-=|\*=|\/=|%=|&&|\|\|)/)) {
      return 'operator';
    }
    if (stream.match(/[+\-*/%=<>!&|^~]/)) return 'operator';

    // ----- Punctuation -----
    if (stream.match(/[(){}\[\];,:.]/)) return 'punctuation';

    // ----- Whitespace -----
    if (stream.eatSpace()) return null;

    // ----- Fallback: skip unrecognized character -----
    stream.next();
    return null;
  },
  languageData: {
    commentTokens: {
      line: '//',
      block: { open: '/*', close: '*/' },
    },
  },
});

// ---------- Public API ----------
export function nuzoLanguage() {
  return new LanguageSupport(nuzoStreamParser);
}

export { nuzoStreamParser };
