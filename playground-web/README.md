# Nuzo Playground Web

> Nuzo Lang 在线试用环境前端项目。

## 概述

基于 Vite + 原生 JavaScript 的轻量级前端，配合 `nuzo-playground-wasm` crate 提供浏览器内编辑 + 运行 Nuzo 脚本的能力。

## 项目结构

```
playground-web/
├── package.json
├── vite.config.js
├── index.html
├── src/
│   ├── main.js              # 入口（T8 基础版）
│   ├── styles.css           # 暗色主题样式
│   └── worker/
│       └── nuzo.worker.js   # Worker 入口（T10 实现，T8 占位）
├── public/                  # 静态资源（如有）
└── README.md
```

## 技术栈

| 项 | 版本 | 用途 |
|----|------|------|
| Vite | ^5.4.0 | 构建工具 + dev server |
| CodeMirror state | ^6.4.0 | 编辑器状态管理（T9 集成） |
| CodeMirror view | ^6.28.0 | 编辑器视图（T9 集成） |
| CodeMirror lang-javascript | ^6.2.0 | 语言描述参考（T9 集成） |

**不引入** React/Vue 等框架，保持简洁与原生 JS。

## 开发

```bash
cd playground-web
npm install
npm run dev      # 启动 dev server，默认 http://localhost:5173
npm run build    # 构建到 dist/
npm run preview  # 预览构建结果
```

## 部署

GitHub Pages 部署由 `.github/workflows/playground-pages.yml` 自动完成：

1. push 到 main 分支（修改 `crates/nuzo-playground-wasm/**` 或 `playground-web/**` 时触发）
2. CI 自动构建 wasm + web：`wasm-pack build` → `npm run build`
3. 部署到 `https://<owner>.github.io/nuzo_lang/`

也支持在 GitHub Actions 页面手动触发（`workflow_dispatch`）。

### 首次部署前需要在 GitHub 仓库 Settings → Pages 中：

- **Source**: 选择 `GitHub Actions`（而非 Deploy from a branch）

> 首次 push 触发 workflow 后，可能需要等待 1-2 分钟 Pages 才可访问。
> 同一时间只允许一个 deployment 运行（`concurrency: group: pages`），新的 push 会取消上一次未完成的部署。

## 设计决策

- **暗色主题为默认**：与代码编辑器搭配，符合开发者习惯
- **响应式布局**：桌面端左编辑器 + 右输出；移动端上下堆叠
- **原生 JS**：不引入框架，降低复杂度与包体积
- **品牌色**：取自 `assets/logo.svg`（蓝 #3b82f6 + 琥珀 #f97316）
- **Worker 隔离**：wasm 在 Worker 中运行，避免阻塞 UI（T10 实施）
- **相对路径构建**：`base: './'` 便于 GitHub Pages 部署

## 后续任务衔接

| 任务 | 衔接点 |
|------|--------|
| T9（CodeMirror 6 集成） | 挂载到 `#editor`；入口 `src/main.js` |
| T10（Web Worker 集成） | 实现 `src/worker/nuzo.worker.js`；`runBtn` 改为 `worker.postMessage` |
| T11（GitHub Pages CI） | `npm run build` 输出 `dist/`，配合 `wasm-pack build --target web` |

## 约束

- 本项目**不属于** Rust workspace，未在根 `Cargo.toml` 的 members 中注册
- wasm 由 Worker 加载，`vite.config.js` 中 `optimizeDeps.exclude: ['nuzo-playground-wasm']`
