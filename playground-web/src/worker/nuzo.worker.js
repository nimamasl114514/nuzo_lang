// ============================================================
// T10: Nuzo Web Worker
//
// 在独立线程中加载并运行 Nuzo wasm，避免主线程阻塞。
//
// 通信协议（与 src/main.js 配对）：
//   主线程 → Worker:
//     { type: 'init' }                                  — 初始化 wasm
//     { type: 'run', payload: { source: string } }      — 运行源码
//
//   Worker → 主线程:
//     { type: 'ready' }                                 — 初始化完成
//     { type: 'result', payload: { success, stdout, diagnostics } }
//     { type: 'error', payload: { message: string } }   — Worker 内异常
//
// wasm 模块位置（CI 已复制）：
//   playground-web/wasm/nuzo_playground_wasm.js
//   playground-web/wasm/nuzo_playground_wasm_bg.wasm
//
// Worker 文件位置：playground-web/src/worker/nuzo.worker.js
// 相对路径：从 src/worker/ 到 wasm/ 需上溯两级（../../wasm/）
// ============================================================

let playground = null;
let initialized = false;

/**
 * 加载并初始化 wasm 模块。
 *
 * wasm-pack --target web 生成的 JS 模块默认导出 init 函数，
 * 调用 init() 后才会加载 .wasm 二进制并实例化。
 * Playground / RunResult / Diagnostic 作为命名导出。
 *
 * 幂等：重复调用直接返回已创建的 playground 实例。
 */
async function initWasm() {
  if (initialized) return playground;

  // 动态 import：Vite 在 worker 中保留 dynamic import 语义
  const wasm = await import('../../wasm/nuzo_playground_wasm.js');
  await wasm.default(); // 加载并实例化 wasm 二进制

  playground = new wasm.Playground();
  initialized = true;
  return playground;
}

/**
 * 将 wasm-bindgen RunResult 序列化为纯 JS 对象。
 *
 * 必要性：wasm-bindgen 对象（Diagnostic 实例等）无法直接通过
 * postMessage 结构化克隆跨线程传递，需手动提取字段到 plain object。
 *
 * 字段映射（lib.rs 的 #[wasm_bindgen(getter)] 暴露为 JS 属性）：
 *   result.success      → bool
 *   result.stdout       → string
 *   result.diagnostics  → Array<Diagnostic>
 *     d.code / d.message / d.severity / d.file
 *     d.line / d.column (u32)
 *     d.source_snippet / d.suggestion
 */
function serializeRunResult(result) {
  return {
    success: result.success,
    stdout: result.stdout,
    diagnostics: result.diagnostics.map((d) => ({
      code: d.code,
      message: d.message,
      severity: d.severity,
      file: d.file,
      line: d.line,
      column: d.column,
      source_snippet: d.source_snippet,
      suggestion: d.suggestion,
    })),
  };
}

self.addEventListener('message', async (e) => {
  const { type, payload } = e.data;

  if (type === 'init') {
    try {
      await initWasm();
      self.postMessage({ type: 'ready' });
    } catch (err) {
      self.postMessage({
        type: 'error',
        payload: { message: `Init failed: ${err && err.message ? err.message : String(err)}` },
      });
    }
    return;
  }

  if (type === 'run') {
    try {
      if (!initialized) {
        await initWasm();
      }

      const { source } = payload;
      const result = playground.run(source);

      self.postMessage({
        type: 'result',
        payload: serializeRunResult(result),
      });
    } catch (err) {
      self.postMessage({
        type: 'error',
        payload: { message: `Run failed: ${err && err.message ? err.message : String(err)}` },
      });
    }
    return;
  }

  // 未知消息类型：忽略但记录日志，便于调试
  console.warn('[T10 Worker] 未知消息类型:', type);
});

console.log('[T10] Nuzo Worker script loaded');
