/**
 * Nuzo Lang 轻量词法分析器
 *
 * 将源码切分为 Token，供 LSP 的补全 / 悬停 / 快速语法检查使用。
 * 不构建完整 AST，仅做 token 分类与位置记录。
 */

export enum TokenKind {
  Keyword,
  Number,
  String,
  Comment,
  Operator,
  Punctuation,
  Identifier,
  Whitespace,
  Eof,
}

export interface Token {
  kind: TokenKind;
  text: string;
  line: number; // 0-based
  col: number; // 0-based
  start: number; // 字符偏移
  end: number;
}

const KEYWORDS = new Set([
  // 英文关键字
  'fn', 'return', 'import', 'as', 'lazy',
  'if', 'else', 'while', 'for', 'in', 'loop',
  'break', 'continue', 'match',
  'try', 'catch', 'out', 'keep',
  'and', 'or', 'not',
  'true', 'false', 'nil',
  // 中文关键字
  '函数', '返回', '导入', '作为', '懒',
  '如果', '否则', '当', '遍历', '在', '循环',
  '跳出', '继续', '匹配',
  '尝试', '捕获', '抛出', '始终',
  '并且', '或者', '非',
  '真', '假', '空',
]);

// 多字符运算符（按长度降序排列，优先匹配长的）
const MULTI_CHAR_OPERATORS = [
  '===', '!==', // 预留
  '==', '!=', '<=', '>=', '&&', '||',
  '+=', '-=', '*=', '/=', '%=',
  '->', '=>',
];
const SINGLE_CHAR_OPERATORS = new Set([
  '+', '-', '*', '/', '%',
  '<', '>', '!', '=',
]);
const PUNCTUATION = new Set([
  '.', ',', ';', ':', '(', ')', '[', ']', '{', '}',
]);

export function tokenize(src: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  let line = 0;
  let col = 0;
  const len = src.length;

  while (i < len) {
    const start = i;
    const ch = src[i];

    // 换行
    if (ch === '\n') {
      tokens.push({ kind: TokenKind.Whitespace, text: ch, line, col, start, end: i + 1 });
      i++; line++; col = 0;
      continue;
    }
    // 空白
    if (ch === ' ' || ch === '\t' || ch === '\r') {
      let j = i;
      while (j < len && (src[j] === ' ' || src[j] === '\t' || src[j] === '\r')) j++;
      tokens.push({ kind: TokenKind.Whitespace, text: src.slice(i, j), line, col, start, end: j });
      col += j - i; i = j;
      continue;
    }
    // 单行注释 //
    if (ch === '/' && src[i + 1] === '/') {
      let j = i + 2;
      while (j < len && src[j] !== '\n') j++;
      tokens.push({ kind: TokenKind.Comment, text: src.slice(i, j), line, col, start, end: j });
      col += j - i; i = j;
      continue;
    }
    // 多行注释 /* */
    if (ch === '/' && src[i + 1] === '*') {
      let j = i + 2;
      let newLine = 0;
      while (j < len && !(src[j] === '*' && src[j + 1] === '/')) {
        if (src[j] === '\n') newLine++;
        j++;
      }
      if (j < len) j += 2; // 跳过 */
      tokens.push({ kind: TokenKind.Comment, text: src.slice(i, j), line, col, start, end: j });
      // 如果跨行，更新 line/col（这里简化处理）
      if (newLine > 0) {
        line += newLine;
        const lastNl = src.lastIndexOf('\n', j - 1);
        col = lastNl >= 0 ? j - lastNl - 1 : j - i;
      } else {
        col += j - i;
      }
      i = j;
      continue;
    }
    // 数字字面量
    if (isDigit(ch) || (ch === '0' && (src[i + 1] === 'x' || src[i + 1] === 'o' || src[i + 1] === 'b'))) {
      let j = i;
      // 十六进制/八进制/二进制
      if (src[i] === '0' && (src[i + 1] === 'x' || src[i + 1] === 'X')) {
        j = i + 2;
        while (j < len && isHexDigit(src[j])) j++;
      } else if (src[i] === '0' && (src[i + 1] === 'o' || src[i + 1] === 'O')) {
        j = i + 2;
        while (j < len && isOctDigit(src[j])) j++;
      } else if (src[i] === '0' && (src[i + 1] === 'b' || src[i + 1] === 'B')) {
        j = i + 2;
        while (j < len && isBinDigit(src[j])) j++;
      } else {
        // 十进制（可能含浮点）
        while (j < len && isDigit(src[j])) j++;
        // 浮点小数部分
        if (src[j] === '.' && isDigit(src[j + 1])) {
          j++;
          while (j < len && isDigit(src[j])) j++;
        }
        // 指数部分 e10 / e-3
        if (src[j] === 'e' || src[j] === 'E') {
          let k = j + 1;
          if (src[k] === '+' || src[k] === '-') k++;
          if (isDigit(src[k])) {
            j = k;
            while (j < len && isDigit(src[j])) j++;
          }
        }
      }
      tokens.push({ kind: TokenKind.Number, text: src.slice(i, j), line, col, start, end: j });
      col += j - i; i = j;
      continue;
    }
    // 字符串 "..."
    if (ch === '"') {
      let j = i + 1;
      while (j < len && src[j] !== '"') {
        if (src[j] === '\\' && j + 1 < len) j += 2;
        else if (src[j] === '\n') break; // 未闭合
        else j++;
      }
      if (j < len && src[j] === '"') j++; // 闭合引号
      tokens.push({ kind: TokenKind.String, text: src.slice(i, j), line, col, start, end: j });
      col += j - i; i = j;
      continue;
    }
    // 字符 '...'
    if (ch === "'") {
      let j = i + 1;
      while (j < len && src[j] !== "'") {
        if (src[j] === '\\' && j + 1 < len) j += 2;
        else if (src[j] === '\n') break;
        else j++;
      }
      if (j < len && src[j] === "'") j++;
      tokens.push({ kind: TokenKind.String, text: src.slice(i, j), line, col, start, end: j });
      col += j - i; i = j;
      continue;
    }
    // 模板字符串 `...${...}...`
    if (ch === '`') {
      let j = i + 1;
      while (j < len && src[j] !== '`') {
        if (src[j] === '\\' && j + 1 < len) j += 2;
        else if (src[j] === '\n') { line++; col = 0; j++; continue; }
        else j++;
      }
      if (j < len && src[j] === '`') j++;
      tokens.push({ kind: TokenKind.String, text: src.slice(i, j), line, col, start, end: j });
      col += j - i; i = j;
      continue;
    }
    // 标识符和关键字
    if (isIdentStart(ch)) {
      let j = i + 1;
      while (j < len && isIdentPart(src[j])) j++;
      const text = src.slice(i, j);
      const kind = KEYWORDS.has(text) ? TokenKind.Keyword : TokenKind.Identifier;
      tokens.push({ kind, text, line, col, start, end: j });
      col += j - i; i = j;
      continue;
    }
    // 多字符运算符
    let matched = false;
    for (const op of MULTI_CHAR_OPERATORS) {
      if (src.startsWith(op, i)) {
        tokens.push({ kind: TokenKind.Operator, text: op, line, col, start, end: i + op.length });
        col += op.length; i += op.length;
        matched = true;
        break;
      }
    }
    if (matched) continue;
    // 单字符运算符
    if (SINGLE_CHAR_OPERATORS.has(ch)) {
      tokens.push({ kind: TokenKind.Operator, text: ch, line, col, start, end: i + 1 });
      col++; i++;
      continue;
    }
    // 标点符号
    if (PUNCTUATION.has(ch)) {
      tokens.push({ kind: TokenKind.Punctuation, text: ch, line, col, start, end: i + 1 });
      col++; i++;
      continue;
    }
    // 未知字符：作为 Identifier 跳过（容错）
    tokens.push({ kind: TokenKind.Identifier, text: ch, line, col, start, end: i + 1 });
    col++; i++;
  }
  tokens.push({ kind: TokenKind.Eof, text: '', line, col, start: i, end: i });
  return tokens;
}

function isDigit(c: string): boolean { return c >= '0' && c <= '9'; }
function isHexDigit(c: string): boolean { return isDigit(c) || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F'); }
function isOctDigit(c: string): boolean { return c >= '0' && c <= '7'; }
function isBinDigit(c: string): boolean { return c === '0' || c === '1'; }
function isIdentStart(c: string): boolean { return /[a-zA-Z_]/.test(c) || /[\u0080-\uFFFF]/.test(c); }
function isIdentPart(c: string): boolean { return isIdentStart(c) || isDigit(c); }
