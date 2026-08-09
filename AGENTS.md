# Repository Guidelines

## Project Overview

**QuickClipboard** — 跨平台剪贴板管理工具。Tauri 2 + Rust 后端，React/Vite 前端。支持 Windows + Android（Tauri mobile）。

Core features: clipboard history with multi-format capture (text, RTF, HTML, image, files), favorites & groups, quick-paste overlay, WebDAV sync with ChaCha20Poly1305 encryption, LAN peer-to-peer transfer, image library. 截屏与贴图（含 GPU 图片查看）功能已移除（私有插件剥离，2026-08）。

---

## Architecture & Data Flow

### 整体分层

```
Frontend (React 19 / Vite 7 / UnoCSS / 纯 JSX)
  │  invoke("command_name", { args })
  ▼
Tauri IPC Layer  (commands/ — #[tauri::command] 模块)
  │  direct function calls
  ▼
Services Layer  (services/ — 编译模块: clipboard, database, paste, sync, WebDAV, LAN transfer 等)
  │  with_connection(|conn| { ... })
  ▼
SQLite  (单连接, parking_lot::Mutex + once_cell::sync::Lazy, WAL mode)
```

### 源码结构

```
src/              ← Vite 根目录（纯 JS/JSX，禁用 TypeScript）
  windows/        ← 独立 mini-app（每个对应一个 Tauri 窗口）
  shared/         ← 跨窗口共享代码（i18n、store、hooks、utils）
  plugins/        ← 前端插件（context_menu、input_dialog）
src-tauri/        ← Rust 后端（Tauri 2）
  src/
    commands/     ← Tauri invoke 命令模块
    services/     ← 业务逻辑（SQLite、clipboard、sync 等）
    windows/      ← 窗口生命周期管理
    utils/        ← 工具函数（screen, positioning, icon, text, html 等）
    security/     ← WebView2 环境变量安全审计
    maintenance/  ← 独立 TUI 维护模式（ratatui + crossterm）
  plugins/        ← Rust 插件（low-memory-fltk）
  capabilities/   ← Tauri 2 权限 JSON（每个窗口模式一个）
```

### 核心架构决策

- **No Tauri `manage()` state** — 全局状态全部使用模块级 `Lazy<Mutex<T>>` 或 `Lazy<RwLock<T>>` 静态变量
- **No ORM** — raw SQL via `rusqlite::params![]`；ad-hoc migration via `PRAGMA table_info` / `PRAGMA user_version`
- **Window-as-mini-app** — 每个窗口是独立 Vite 入口点，共享代码在 `src/shared/`
- **No frontend router** — 导航通过 React state，不是 URL 路由
- **Valtio** 前端状态管理 — `proxy()` stores + `useSnapshot()` hooks；方法直接挂在 proxy 对象上

### 线程模型

| 线程 | 用途 |
|---|---|
| Main thread | Tauri event loop, non-blocking commands |
| Clipboard monitor | `std::thread` — 长期阻塞的 `clipboard-rs` 监听 |
| tokio runtime（multi-thread） | 后台任务：LAN sync server, WebDAV scheduler, crypto |
| `spawn_blocking` | SQLite 查询、OCR、文件 I/O（从 async command 中） |

### 窗口间通信

- 前端 → 前端: `@tauri-apps/api/event`（`emit`/`listen`）
- 前端 → 后端: `invoke()` from `@tauri-apps/api/core`
- 后端 → 前端: Tauri events（`app_handle.emit()`）

**各层详细规范与代码约定：** 参见各子目录下的 `AGENTS.md`：
- `src-tauri/src/AGENTS.md` — Rust 架构决策、错误处理、数据库、异步模式、全局状态、反模式
- `src/AGENTS.md` — 前端窗口架构、通信、状态管理、组件规范、样式
- `src/shared/AGENTS.md` — 共享模块：API 封装、stores、hooks、i18n 协议、事件监听

**扩展文档：** 参见 `docs/常用命令.md`、`docs/工作树配置.md`（注：`docs/` 在 `.gitignore` 中，需本地生成，不在 git 中分发）。

---

## Key Directories

| 路径 | 用途 |
|---|---|
| `src/` | Vite 根目录 — 纯 JS/JSX 前端（禁用 TypeScript） |
| `src/windows/` | 独立 mini-app，每个一个 Tauri 窗口 |
| `src/shared/` | 跨窗口代码：stores (Valtio), hooks, i18n, API wrappers, UI 组件, utils |
| `src/shared/api/` | `invoke()` 封装层 — 每个后端命令对应一个 async 函数（完整列表: `lsp symbols`） |
| `src/shared/store/` | Valtio proxy stores（完整列表: `lsp symbols`） |
| `src/shared/hooks/` | 共享 React hooks（完整列表: `lsp symbols`） |
| `src/shared/components/ui/` | 基础 UI 组件（完整列表: `lsp symbols`） |
| `src/shared/locales/` | i18n 翻译文件（zh-CN.json, en-US.json） |
| `src/plugins/` | 前端窗口插件：`context_menu`, `input_dialog` |
| `src-tauri/` | Rust 后端 |
| `src-tauri/src/commands/` | `#[tauri::command]` IPC handlers（详见 `commands/mod.rs`） |
| `src-tauri/src/services/` | 业务逻辑（完整列表: `lsp symbols` 或 `services/mod.rs`） |
| `src-tauri/src/services/clipboard/` | monitor（SHA256 去重 + atomic pause）, capture（多格式）, processor（分类 + 来源识别） |
| `src-tauri/src/services/database/` | SQLite 单连接（Mutex + Lazy），models, WAL pragma |
| `src-tauri/src/services/webdav_sync/` | client, uploader, downloader, crypto, scheduler, cloud files 等 |
| `src-tauri/src/services/sync_transfer/` | LAN P2P：HTTP server/client, UDP discovery, pairing, auto-sync |
| `src-tauri/src/windows/` | Rust 窗口生命周期管理（详见 `windows/mod.rs`） |
| `src-tauri/src/utils/` | 工具：screen geometry, cursor positioning, icon 提取, text/html 处理 |
| `src-tauri/plugins/` | Rust 插件：`low-memory-fltk` |
| `src-tauri/capabilities/` | Tauri 2 权限 JSON 文件（每个窗口模式一个） |
| `scripts/` | 构建编排：community build/check, dev launcher, cleanup, README updater |
| `docs/` | 中文命令参考、工作树配置、bug 记录（gitignored） |
| `i18n/` | 本地化 README（en, ja, ko, zh-TW） |

---

## Development Commands

### 环境要求

- Node.js ≥ 16（`package.json` 中未声明 `engines` 字段，此为推荐值）
- Rust ≥ 1.70 + tauri-cli ≥ 2.0

### 前端（仅 Vite）

```bash
npm install                        # 安装依赖
npm run dev                        # Vite dev server on port 1421
npm run build                      # Vite 生产构建（不含 Tauri）
```

### 开发与构建（单一形态，无私有插件）

```bash
npm run tauri dev                  # 开发模式
npm run tauri:dev:no-watch         # 开发模式（禁用文件监听）
npm run tauri:build                # 正式构建
npm run tauri:dev:community        # 兼容脚本（同 tauri dev）
npm run tauri:build:community      # 兼容脚本（同 tauri build）
npm run check:community            # cargo check
npm run clippy:community           # cargo clippy
npm run test:rust                  # cargo test
```

> `npm run dev` / `npm run build` 仅运行 Vite（不含 Tauri）。除非只需要前端打包，否则使用 `tauri dev` / `tauri build`。

### 环境变量

| 变量 | 效果 |
|---|---|
| `QC_COMMUNITY=1` | 兼容标志（无实际效果，脚本保留） |
| `NODE_ENV=development` | 控制 minification, sourcemaps, esbuild console 去除 |
| `TAURI_DEBUG=true` | 同 `NODE_ENV=development`（由 Tauri dev channel 自动设置） |

---

## Code Conventions & Common Patterns

详细的代码约定和示例分布在各层 AGENTS.md 中：

| 层 | 文档 | 内容 |
|---|---|---|
| Rust 后端 | `src-tauri/src/AGENTS.md` | 错误处理、数据库访问、全局状态、async 模式、Serde 约定、feature gate、SQLite 约束 |
| 前端整体 | `src/AGENTS.md` | 窗口架构、前后端通信、Valtio 状态管理、组件规范、UnoCSS 样式、i18n/DOMPurify |
| 共享模块 | `src/shared/AGENTS.md` | API 封装细节、stores、hooks、事件监听、i18n 协议 |

### 命名约定（全局）

| 层 | 约定 | 示例 |
|---|---|---|
| Rust commands | `snake_case` | `get_clipboard_history`, `paste_content`, `save_settings` |
| JS API wrappers | `camelCase` | `getClipboardHistory()`, `pasteContent()`, `saveSettings()` |
| JS 文件 | `camelCase` 模块, `PascalCase` 组件 | `settingsStore.js`, `Toggle.jsx` |
| 窗口目录 | `camelCase` | `quickpaste/`, `textEditor/`, `pinImage/` |
| i18n keys | `dot.separated.snake_like` | `settings.shortcuts.tabs.globalHotkey` |

---

## Important Files

### 入口 & 启动

| 文件 | 作用 |
|---|---|
| `src-tauri/src/main.rs` | Binary entry — 分发维护模式 或 `lib::run()` |
| `src-tauri/src/lib.rs` | Crate root — `run()`: 构建 Tauri app, 注册所有 commands, `setup()` 初始化所有子系统 |
| `src-tauri/build.rs` | `tauri_build::build()` |
| `src-tauri/src/startup_diagnostics.rs` | Panic hook, 启动阶段追踪, 上次实例检测 |
| `src-tauri/src/security/mod.rs` | WebView2 env var 安全审计（release builds） |
| `vite.config.js` | Vite 7 config — 多入口, aliases, UnoCSS, React Compiler, code splitting |
| `src/shared/i18n.js` | i18next 初始化, locale 加载 |
| `src/windows/main/index.jsx` | 前端入口 — store init, React root, event listeners 设置 |

### 配置

| 文件 | 内容 |
|---|---|
| `package.json` | JS deps (React 19, Vite 7, UnoCSS, Valtio, i18next, CodeMirror, Pixi.js, @dnd-kit) + scripts |
| `src-tauri/Cargo.toml` | Rust deps, feature flags (`default = []`, `custom-protocol`), release profile (LTO=fat, opt-level=3, panic=abort) |
| `src-tauri/tauri.conf.json` | Tauri 2 app config — main window (360×520, transparent, always-on-top, skip taskbar, decorations=false), capability refs, NSIS installer, updater endpoint |
| `uno.config.js` | UnoCSS presets, 自定义主题 token, 动态颜色规则, shortcuts |
| `src-tauri/capabilities/*.json` | 窗口作用域权限文件（每个窗口类型一个） |

### 关键模块（复杂度最高 / 改动最频繁）

| 模块 | 说明 |
|---|---|
| `src-tauri/src/commands/clipboard.rs` | Clipboard CRUD, paste, 多格式捕获 |
| `src-tauri/src/commands/settings.rs` | Settings 持久化, 快捷键, 窗口位置 |
| `src-tauri/src/services/settings/model.rs` | `AppSettings` struct — 全部用户配置 |
| `src-tauri/src/services/clipboard/monitor.rs` | Clipboard 监听, atomic pause, SHA256 去重 |
| `src-tauri/src/services/clipboard/processor.rs` | 内容分类 + 来源应用识别（Windows） |
| `src-tauri/src/services/webdav_sync/` | 完整 WebDAV 同步链：client, upload, download, 加密, scheduler |
| `src-tauri/src/services/sync_transfer/` | LAN P2P：UDP 发现, HTTP server/client, pairing, auto-sync |
| `src/windows/main/App.jsx` | 主窗口：tab 导航, 搜索, 多选, 窗口事件 |
| `src/windows/quickpaste/App.jsx` | 快速粘贴浮窗, 虚拟列表 (react-virtuoso) |
| `src/windows/preview/App.jsx` | 内容预览窗口，多种视图模式 |
| `src/shared/store/settingsStore.js` | 配置项 load/save/update 方法 |
| `src/shared/store/clipboardStore.js` | 剪贴板列表 + 虚拟列表缓存 |

---

## Runtime/Tooling Preferences

### 必需

| 工具 | 约束 |
|---|---|
| **Node.js** | ≥ 16（`package.json` 无 `engines` 字段） |
|**Rust**|≥ 1.70 + `tauri-cli` ≥ 2.0 |
|**包管理器**|**npm only** 用于正式构建。`bun` 开发时可用且速度更快，但 **正式构建必须用 npm** — bun 的 symlink 扁平化结构会导致 Vite 构建产物在 transparent WebView2 下触发渲染异常。有 `package-lock.json`。 |
| **OS** | Primary target: Windows。同时支持 macOS, Linux（桌面端）, Android（移动端） |

### 构建工具

- **Vite 7** — dev server + bundler, root=`src/`, port 1421
- **UnoCSS** — global mode（不用 CSS Modules），presetUno + presetAttributify + presetIcons
- **Babel** — 仅用于 `babel-plugin-react-compiler`（React Compiler，自动 memoize 符合规则的组件）
- **esbuild** — minification + 在生产构建中去除 `console.log`/`.info`/`.debug`
- **Tauri CLI 2** — `cargo tauri dev` / `cargo tauri build`

### 不存在的工具

- 无 ESLint / Prettier / EditorConfig — 格式化靠约定
- 无 pre-commit hooks（无 husky, lint-staged）
- 无 Makefile / justfile / Taskfile
- 无 `.env` 文件

### IDE

```json
// .vscode/extensions.json
{ "recommendations": ["tauri-apps.tauri-vscode", "rust-lang.rust-analyzer"] }
```

---

## Testing & QA

### 测试框架

| 层 | 框架 | 覆盖范围 |
|---|---|---|
| Rust 后端 | `cargo test`（仅单元测试） | 内联 `#[cfg(test)]` 模块 |
|前端|**无**（项目级）| `package.json` 中无 Vitest/Jest/Playwright |

无 `tests/` 集成测试目录，无 `[dev-dependencies]` in `Cargo.toml`，无 coverage 工具或阈值。

### 运行测试

```bash
npm run test:rust          # cargo test
npm run check:community    # cargo check
npm run clippy:community   # cargo clippy
```

### 测试架构

所有 Rust 测试都是 `#[cfg(test)] mod tests { ... }` 内联在源文件中。测试验证：

- Clipboard content type 检测和去重
- Database models（group color normalization, merge logic）
- Settings migration（blacklist → blocklist）
- Paste merge logic（mixed format rejection, plain-text separators, rich-text wrapping）
- WebDAV crypto（encrypt/decrypt, anti-tampering via HMAC）
- Window positioning（rect clamping to monitor bounds）
- Startup/registry commands（Windows 专有）

### 社区测试脚本原理

`scripts/community-check.js` 直接运行 `cargo test`（或 `check`/`clippy`），私有插件已剥离，无补丁机制。

### CI Pipeline

单一 workflow（`.github/workflows/release.yml`）— Windows-latest，tag/release/manual 触发。构建 NSIS 安装程序 + portable EXE，生成 updater JSONs，上传至阿里云 OSS CDN。**CI 不执行测试套件** — 流程只构建，不运行测试。

---

## 版本形态

私有插件（`gpu-image-viewer`、`screenshot-suite`）已于 2026-08 剥离（截屏、贴图、GPU 图片查看功能移除），仓库为单一纯 OSS 形态，无完整版/社区版之分。`QC_COMMUNITY`、`tauri:dev:community`、`tauri:build:community`、`check:community` 等保留为兼容项。

`Cargo.toml` 中的 feature 标志：
- `default = []`
- `custom-protocol = ["tauri/custom-protocol"]` — 协议资产功能
- Windows-only 依赖（`qcocr`、`winreg`）声明在 `[target.'cfg(windows)'.dependencies]`，OCR 命令与图片库 OCR 文件名逻辑通过 `#[cfg(windows)]` 门控

### 构建脚本

| 脚本 | 作用 |
|---|---|
| `scripts/community-build.js` | 构建脚本（兼容参数：`--dev` / `--full` / `--no-default-features`） |
| `scripts/community-check.js` | Rust 检查 — 执行 `cargo check/clippy/test` |
| `scripts/build-tauri.js` | 正式构建 — `npm run build` |
| `scripts/dev-tauri.js` | 开发模式 — `npm run dev` |
| `scripts/ensure-clean-workspace.js` | 兼容占位（私有补丁机制已移除） |
| `scripts/update-readme-downloads.js` | 发版后更新所有 README 下载链接和版本号 |


## 关键约束

| 约束 | 理由 |
|---|---|
| **禁止 TypeScript** — `src/` 下只能使用纯 JS/JSX | 项目约定 |
| **Valtio** — 唯一的状态管理方案（禁止 Redux/Zustand/MobX/Jotai） | 项目约定 |
| **UnoCSS global 模式** — 使用 utility class，不用 CSS Modules | 样式方案 |
| **i18n** — `zh-CN.json` 和 `en-US.json` 必须同步更新 | 强制双语覆盖 |
| **SQLite** — 单连接 + `Mutex`，WAL 模式，不支持并发写入 | 架构限制 |
| **React Compiler** — 通过 `babel-plugin-react-compiler` 激活，遵循 Rules of React | 性能 |
| **npm for production** — `bun` 在 transparent WebView2 下有渲染 bug | 已知问题 |
| **Capability-per-window** — 每个窗口类型有独立的 capability JSON，最小权限原则 | 安全 |
| `CSP = null` in `tauri.conf.json` — 放宽以支持 asset protocol | WebView2 兼容 |
| `.gitignore` 排除项：`docs/` | — |
| **PR 推送** — 强制推送必须先 `checkout` 到 PR 分支，再 `cherry-pick` 需要的提交后 `--force-with-lease`，禁止直接从 `dev` 强推到 PR 分支 | 防止覆盖原有结构 |

---

## 添加功能

### 新增 Tauri 窗口（前端 + 后端）

1. 创建 `src/windows/<name>/` 目录，含 `index.html`、`index.jsx`、`App.jsx`
2. 在 `vite.config.js` → `rollupOptions.input` 中添加条目
3. 在 `src-tauri/src/windows/` 中创建 Rust 窗口管理器，使用 `WebviewWindowBuilder` 动态创建窗口
4. 在 `src-tauri/capabilities/<name>.json` 中添加 capabilities 文件
5. 在 `src-tauri/tauri.conf.json` → `app.security.capabilities` 中注册 capability 名称

> ⚠️ 多数窗口由 Rust 代码**动态创建**（`WebviewWindowBuilder::new()`），不出现在 `tauri.conf.json` 的 `app.windows` 数组中。`app.windows` 中仅 `main` 窗口（因其需要特殊的窗口属性如透明、无边框、总在最上等）。Multi-instance 窗口（`text-editor-*`, `pin-image-*`, `transfer-shelf-*`）在 capabilities 中使用通配符匹配。

### 新增 Tauri 命令（后端）

1. 创建 `commands/<name>.rs`，实现 `#[tauri::command]` 函数
2. 在 `commands/mod.rs` 中注册：`pub mod <name>` + `pub use <name>::*`
3. 添加到 `lib.rs` 的 `generate_handler![]` 列表中
4. 添加到对应的 capabilities JSON
5. 创建对应前端封装 `src/shared/api/<name>.js`

**漏掉任何一步 → `invoke()` 静默失败（无编译错误）。**

### 新增 i18n 字符串

在 `src/shared/locales/zh-CN.json` 和 `en-US.json` 中**同时**添加 key。不可省略任何一个。


---


## LSP 实时查询

代码结构（模块列表、符号引用、调用关系）优先通过 LSP 获取实时数据，本文档中的静态枚举仅供参考。常用查询：

| 想了解 | LSP 命令 |
|---|---|
| 所有 command 模块 | `lsp symbols file:"src-tauri/src/commands"` |
| 所有 service 模块 | `lsp symbols file:"src-tauri/src/services"` |
| 所有前端 API 模块 | `lsp symbols file:"src/shared/api"` |
| 所有 hooks | `lsp symbols file:"src/shared/hooks"` |
| 某个函数的调用者 | `lsp references file:"<path>" symbol:"<name>"` |
| 某个符号的定义 | `lsp definition file:"<path>" symbol:"<name>"` |
| 跨文件重命名 | `lsp rename file:"<path>" symbol:"<name>" new_name:"<new>"` |

各子目录 `AGENTS.md` 中也有针对性的 LSP 查询示例。
