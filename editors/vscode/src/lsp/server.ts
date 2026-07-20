/**
 * Nuzo LSP Server 入口。
 *
 * 职责：
 *  - 响应 initialize/initialized，声明 completion/hover/definition 能力
 *  - 文档变更时：本地词法快速检查 + 调用 nuzo compile 获取编译器诊断
 *  - onCompletion：基于行前缀的关键字补全
 *  - onHover：基于 token 的关键字文档
 *  - onDefinition：P2 暂不实现
 */

// v9 的 0 参 createConnection 仅在 /node 子路径导出（lib/node/main.d.ts），
// 通用 typings 入口 lib/common/api.d.ts 只导出 2-3 参版本。/node 同时 re-export 全部公共类型。
import { createConnection, TextDocuments, InitializeResult, TextDocumentSyncKind, Diagnostic, DiagnosticSeverity, CompletionItem, Hover, MarkupContent } from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { tokenize, TokenKind } from './lexer';
import { getCompletions, getKeywordDocs } from './completion';
import { parseCompilerErrors } from './diagnostics';
import { execFile } from 'child_process';
import { promisify } from 'util';
import * as path from 'path';
import * as fs from 'fs';

const execFileAsync = promisify(execFile);

const connection = createConnection();
const documents = new TextDocuments(TextDocument);
documents.listen(connection);

connection.onInitialize((_params): InitializeResult => {
  connection.console.info('Nuzo Language Server initializing...');
  return {
    capabilities: {
      textDocumentSync: TextDocumentSyncKind.Incremental,
      completionProvider: { resolveProvider: false, triggerCharacters: ['.', 'f', 'l', 'i', 's', 'e'] },
      hoverProvider: true,
      definitionProvider: true,
    },
  };
});

connection.onInitialized(() => {
  connection.console.info('Nuzo Language Server ready');
});

// 文档变更 → 触发诊断（debounce，避免每次按键都调用编译器）
const validationTimers = new Map<string, NodeJS.Timeout>();
const VALIDATION_DEBOUNCE_MS = 500;

documents.onDidChangeContent((change) => {
  const uri = change.document.uri;
  const existing = validationTimers.get(uri);
  if (existing) clearTimeout(existing);
  validationTimers.set(uri, setTimeout(() => {
    validationTimers.delete(uri);
    validateTextDocument(change.document);
  }, VALIDATION_DEBOUNCE_MS));
});

async function validateTextDocument(doc: TextDocument): Promise<void> {
  const text = doc.getText();
  // 1. 本地词法快速检查（未闭合字符串/注释）
  const localDiags = quickSyntaxCheck(doc, text);
  // 2. 调用 nuzo check 获取编译器诊断
  const compilerDiags = await getCompilerDiagnostics(doc);
  // 合并诊断（去重：相同 range+message 只保留一个）
  const allDiags = mergeDiagnostics(localDiags, compilerDiags);
  connection.sendDiagnostics({ uri: doc.uri, diagnostics: allDiags });
}

function mergeDiagnostics(local: Diagnostic[], compiler: Diagnostic[]): Diagnostic[] {
  const seen = new Set<string>();
  const result: Diagnostic[] = [];
  for (const d of [...local, ...compiler]) {
    const key = `${d.range.start.line}:${d.range.start.character}:${d.message}`;
    if (!seen.has(key)) {
      seen.add(key);
      result.push(d);
    }
  }
  return result;
}

async function getCompilerDiagnostics(doc: TextDocument): Promise<Diagnostic[]> {
  const filePath = doc.uri;
  // 从 LSP URI (file:///...) 转为文件系统路径
  const fsPath = filePath.startsWith('file://') ? filePath.replace('file:///', '') : filePath;
  const executable = findExecutable(fsPath);
  if (!executable) return []; // 找不到可执行文件，静默跳过

  try {
    const { stdout, stderr } = await execFileAsync(executable, ['check', fsPath], {
      cwd: path.dirname(fsPath),
      maxBuffer: 10 * 1024 * 1024,
      timeout: 10_000,
      windowsHide: true,
    });
    // 编译器输出错误到 stderr 或 stdout
    const output = stderr || stdout;
    if (output.trim().length === 0) return []; // 编译成功
    return parseCompilerErrors(output, doc);
  } catch (err: unknown) {
    const e = err as { stderr?: string; stdout?: string };
    const output = e.stderr || e.stdout || '';
    if (output.trim().length === 0) return [];
    return parseCompilerErrors(output, doc);
  }
}

function findExecutable(filePath: string): string | null {
  // 简化版查找：从文件目录向上找 target/debug/nuzo_run.exe
  let dir = path.dirname(filePath);
  for (let i = 0; i < 10; i++) {
    const debugPath = path.join(dir, 'target', 'debug', 'nuzo_run.exe');
    const releasePath = path.join(dir, 'target', 'release', 'nuzo_run.exe');
    if (fs.existsSync(debugPath)) return debugPath;
    if (fs.existsSync(releasePath)) return releasePath;
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return null;
}

function quickSyntaxCheck(doc: TextDocument, text: string): Diagnostic[] {
  const diags: Diagnostic[] = [];
  const tokens = tokenize(text);
  for (const tok of tokens) {
    // 未闭合字符串检测
    if (tok.kind === TokenKind.String && tok.text.length > 0) {
      const first = tok.text[0];
      const last = tok.text[tok.text.length - 1];
      if (first === '"' && last !== '"') {
        diags.push({
          severity: DiagnosticSeverity.Error,
          range: { start: { line: tok.line, character: tok.col }, end: { line: tok.line, character: tok.col + tok.text.length } },
          message: 'Unterminated string literal',
          source: 'nuzo',
        });
      }
    }
  }
  return diags;
}

connection.onCompletion((textDocumentPosition): CompletionItem[] => {
  const doc = documents.get(textDocumentPosition.textDocument.uri);
  if (!doc) return [];
  const text = doc.getText();
  const pos = textDocumentPosition.position;
  // 提取当前行的前缀
  const lines = text.split('\n');
  const line = lines[pos.line] ?? '';
  let prefix = '';
  for (let i = pos.character - 1; i >= 0; i--) {
    const c = line[i];
    if (/[a-zA-Z_]/.test(c)) prefix = c + prefix;
    else break;
  }
  return getCompletions(prefix);
});

connection.onHover((textDocumentPosition): Hover | null => {
  const doc = documents.get(textDocumentPosition.textDocument.uri);
  if (!doc) return null;
  const text = doc.getText();
  const tokens = tokenize(text);
  for (const tok of tokens) {
    if (tok.line === textDocumentPosition.position.line &&
        textDocumentPosition.position.character >= tok.col &&
        textDocumentPosition.position.character < tok.col + tok.text.length) {
      if (tok.kind === TokenKind.Keyword) {
        const docs = getKeywordDocs(tok.text);
        if (docs) {
          const content: MarkupContent = { kind: 'markdown', value: docs };
          return { contents: content };
        }
      }
      break;
    }
  }
  return null;
});

connection.onDefinition(() => null); // P2: 暂不实现

connection.listen();
