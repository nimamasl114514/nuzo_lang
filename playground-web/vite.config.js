import { defineConfig } from 'vite';

export default defineConfig({
  base: './',  // 相对路径，便于 GitHub Pages 部署
  build: {
    target: 'es2022',
    outDir: 'dist',
  },
  // 模块 Worker（new Worker(..., { type: 'module' })）必须用 ES format：
  // IIFE/UMD 不支持 code-splitting，会导致 vite build 报
  // "Invalid value 'iife' for option 'output.format'"
  worker: {
    format: 'es',
  },
  optimizeDeps: {
    exclude: ['nuzo-playground-wasm'],  // wasm 由 worker 加载
  },
  server: {
    port: 5173,
    open: true,
  },
});
