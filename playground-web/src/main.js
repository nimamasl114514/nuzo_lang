// ============================================================
// Nuzo Playground - Main Entry
//
// T8: UI skeleton
// T9: CodeMirror 6 integration with Nuzo syntax highlighting
// T10: Web Worker integration — 运行按钮真正调用 wasm
// ============================================================

import { EditorState } from '@codemirror/state';
import { EditorView, keymap, lineNumbers, highlightActiveLine } from '@codemirror/view';
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
import { syntaxHighlighting, defaultHighlightStyle } from '@codemirror/language';

import { nuzoLanguage } from './nuzo-language.js';

const runBtn = document.getElementById('run-btn');
const clearBtn = document.getElementById('clear-btn');
const output = document.getElementById('output');
const editorContainer = document.getElementById('editor');

// ---------- CodeMirror 6 editor with Nuzo syntax highlighting ----------
const editor = new EditorView({
  state: EditorState.create({
    doc: '// 在此输入 Nuzo 代码\nprint("Hello, Nuzo!")\n',
    extensions: [
      lineNumbers(),
      history(),
      keymap.of([...defaultKeymap, ...historyKeymap]),
      highlightActiveLine(),
      syntaxHighlighting(defaultHighlightStyle),
      nuzoLanguage(),
      EditorView.lineWrapping,
      EditorView.theme({
        '&': { height: '100%', fontSize: '14px' },
        '.cm-scroller': { fontFamily: 'Consolas, Monaco, monospace' },
      }),
    ],
  }),
  parent: editorContainer,
});

// Expose editor globally for T10 Worker integration
window.__nuzoEditor = editor;

// ============================================================
// T10: Web Worker 集成
// ============================================================

// 创建 Worker（Vite 模块 worker 标准模式）
const worker = new Worker(
  new URL('./worker/nuzo.worker.js', import.meta.url),
  { type: 'module' },
);

// Worker 是否已完成 wasm 初始化（ready 之前禁用运行按钮）
let workerReady = false;

// 启动时即发送 init 指令，预加载 wasm
worker.postMessage({ type: 'init' });

// ---------- Worker 消息处理 ----------
worker.addEventListener('message', (e) => {
  const { type, payload } = e.data;

  if (type === 'ready') {
    workerReady = true;
    runBtn.disabled = false;
    runBtn.textContent = '运行 (Ctrl+Enter)';
    console.log('[T10] Worker ready');
    return;
  }

  if (type === 'result') {
    handleRunResult(payload);
    runBtn.disabled = false;
    runBtn.textContent = '运行 (Ctrl+Enter)';
    return;
  }

  if (type === 'error') {
    output.textContent = `[Worker Error] ${payload.message}`;
    switchTab('stderr');
    runBtn.disabled = false;
    runBtn.textContent = '运行 (Ctrl+Enter)';
    console.error('[T10] Worker error:', payload.message);
    return;
  }
});

// 捕获 Worker 未处理异常（脚本错误、加载失败等）
worker.addEventListener('error', (e) => {
  const detail = [
    `[Worker 异常] ${e.message || '未知错误'}`,
    `  file: ${e.filename || '?'}`,
    `  line: ${e.lineno || '?'}`,
  ].join('\n');
  output.textContent = detail;
  switchTab('stderr');
  runBtn.disabled = false;
  runBtn.textContent = '运行 (Ctrl+Enter)';
  console.error('[T10] Worker uncaught error:', e);
});

/**
 * 处理 Worker 返回的 RunResult，更新输出区。
 *
 * - success: 显示 stdout（空输出显示占位），切换到 stdout tab
 * - failure: 格式化 diagnostics 列表，切换到 stderr tab
 *
 * Diagnostic 格式化：
 *   [E0008] Undefined variable 'unknown_var' (line 3:6)
 *     print(unknown_var)
 *     提示: Declare 'unknown_var' before using it, or check for typos...
 */
function handleRunResult({ success, stdout, diagnostics }) {
  if (success) {
    output.textContent = stdout || '(无输出)';
    switchTab('stdout');
    return;
  }

  const errorText = diagnostics
    .map((d) => {
      let line = `[${d.code}] ${d.message}`;
      if (d.line > 0) {
        line += ` (line ${d.line}`;
        if (d.column > 0) line += `:${d.column}`;
        line += ')';
      }
      if (d.source_snippet) {
        line += `\n  ${d.source_snippet}`;
      }
      if (d.suggestion) {
        line += `\n  提示: ${d.suggestion}`;
      }
      return line;
    })
    .join('\n\n');

  output.textContent = errorText || '(无诊断信息)';
  switchTab('stderr');
}

// ---------- 运行按钮 ----------
runBtn.addEventListener('click', () => {
  if (!workerReady) {
    output.textContent = '正在初始化，请稍候...';
    switchTab('stdout');
    return;
  }

  const source = window.__nuzoEditor.state.doc.toString();
  if (!source.trim()) {
    output.textContent = '(代码为空)';
    switchTab('stdout');
    return;
  }

  // 进入"运行中"状态：禁用按钮，等待 Worker 响应
  runBtn.disabled = true;
  runBtn.textContent = '运行中...';
  worker.postMessage({ type: 'run', payload: { source } });
});

clearBtn.addEventListener('click', () => {
  output.textContent = '';
});

// ---------- Tab 切换 ----------
/**
 * 切换输出区 tab 高亮状态。
 *
 * 当前 HTML 中 stdout/stderr 共用同一个 #output 元素，
 * tab 切换仅改变按钮 active 样式（视觉指示当前内容类别）。
 */
function switchTab(tabName) {
  document.querySelectorAll('.tab-btn').forEach((btn) => {
    btn.classList.toggle('active', btn.dataset.tab === tabName);
  });
}

document.querySelectorAll('.tab-btn').forEach((btn) => {
  btn.addEventListener('click', () => {
    switchTab(btn.dataset.tab);
  });
});

// ---------- Ctrl+Enter 快捷键运行 ----------
document.addEventListener('keydown', (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
    e.preventDefault();
    runBtn.click();
  }
});

console.log('[T10] Playground main loaded');
