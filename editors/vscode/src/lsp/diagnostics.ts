/**
 * 编译错误解析：将 nuzo_run compile 的 stderr 输出转换为 LSP Diagnostic。
 *
 * 支持四种输出格式（见 tryParseErrorLine），无法匹配时整体作为一条诊断回退。
 */

import { Diagnostic, DiagnosticSeverity, Range } from 'vscode-languageserver';
import { TextDocument } from 'vscode-languageserver-textdocument';

interface ParsedError {
  line: number;    // 1-based 转为 0-based
  col?: number;    // 1-based 转为 0-based
  message: string;
  code?: string;
}

/**
 * 解析 nuzo_run compile 的 stderr 输出
 * 支持的格式：
 *   1. error[C0001]: <msg> at line N:col M
 *   2. error: <msg> (line N)
 *   3. <file>:N:M: error: <msg>
 *   4. Error at line N: <msg>
 */
export function parseCompilerErrors(stderr: string, doc: TextDocument): Diagnostic[] {
  const diags: Diagnostic[] = [];
  const lines = stderr.split('\n');

  for (const line of lines) {
    const parsed = tryParseErrorLine(line);
    if (parsed) {
      const range = makeRange(doc, parsed.line, parsed.col);
      diags.push({
        severity: DiagnosticSeverity.Error,
        range,
        message: parsed.message,
        source: 'nuzo',
        code: parsed.code,
      });
    }
  }

  // 如果没解析出结构化错误，但有 stderr，整体作为一个诊断
  if (diags.length === 0 && stderr.trim().length > 0) {
    diags.push({
      severity: DiagnosticSeverity.Error,
      range: { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } },
      message: stderr.trim(),
      source: 'nuzo',
    });
  }
  return diags;
}

function tryParseErrorLine(line: string): ParsedError | null {
  // 格式 1: error[C0001]: msg at line N:col M
  let m = line.match(/error(?:\[([^\]]+)\])?:\s*(.+?)\s+at\s+line\s+(\d+)(?::col\s+(\d+))?/i);
  if (m) {
    return { code: m[1], message: m[2], line: parseInt(m[3], 10) - 1, col: m[4] ? parseInt(m[4], 10) - 1 : undefined };
  }
  // 格式 2: error: msg (line N)
  m = line.match(/error:\s*(.+?)\s*\(line\s+(\d+)\)/i);
  if (m) {
    return { message: m[1], line: parseInt(m[2], 10) - 1 };
  }
  // 格式 3: <file>:N:M: error: msg
  m = line.match(/[^:]+:(\d+):(\d+):\s*error:\s*(.+)/);
  if (m) {
    return { message: m[3], line: parseInt(m[1], 10) - 1, col: parseInt(m[2], 10) - 1 };
  }
  // 格式 4: Error at line N: msg
  m = line.match(/Error\s+at\s+line\s+(\d+):\s*(.+)/i);
  if (m) {
    return { message: m[2], line: parseInt(m[1], 10) - 1 };
  }
  return null;
}

function makeRange(doc: TextDocument, line: number, col?: number): Range {
  const lineText = doc.getText({ start: { line, character: 0 }, end: { line, character: 9999 } });
  const startCol = col ?? 0;
  const endCol = col ?? (lineText.length > 0 ? lineText.length : 1);
  return {
    start: { line, character: startCol },
    end: { line, character: Math.max(endCol, startCol + 1) },
  };
}
