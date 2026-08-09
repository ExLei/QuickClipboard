# src/ — 前端

Vite 根目录。纯 JavaScript/JSX，禁用 TypeScript。

> **实时查询：** 模块列表、hooks、stores 的完整清单通过 LSP 获取权威数据。详见文末 [LSP 实时查询](#lsp-实时查询) 章节。

## 结构

```
src/
├── windows/          # 独立 mini-app（每个窗口一个 Vite 入口）
│   ├── main/         #   主窗口（剪贴板历史、收藏、分组、emoji）
│   ├── quickpaste/   #   快速粘贴浮窗（滚轮导航）
│   ├── settings/     #   设置窗口（多个设置 section）
│   ├── preview/      #   内容预览窗口（多视图模式）
│   ├── community/    #   社区版专属窗口
│   ├── textEditor/   #   文本编辑器窗口（CodeMirror 代码编辑 + tiptap 富文本）
│   ├── updater/      #   更新器窗口
│   ├── receiveBox/   #   接收文件窗口
│   ├── transferShelf/ #  传输文件架窗口
├── shared/           # 跨窗口共享模块（详见 shared/AGENTS.md）
├── plugins/          # 前端窗口插件（context_menu, input_dialog）
└── assets/           # 静态资源
```

## 每个窗口的标准文件

```
<窗口>/
├── index.html       # Vite 入口 HTML（被 rollupOptions.input 引用）
├── index.jsx        # React 挂载点，import './index.html'，store init + event listeners
└── App.jsx          # 根组件
```

> 部分既有窗口偏离此模式：
> - `community/` 使用 `index.js`（非 `index.jsx`），且缺少 `App.jsx`
> - `updater/` 缺少 `App.jsx`
>
> 新增窗口时遵循标准模式即可。

## 新增窗口步骤

1. 创建 `src/windows/<name>/` 目录，含标准三文件
2. 在 `vite.config.js` → `rollupOptions.input` 中添加条目
3. 在 `src-tauri/src/windows/` 中创建 Rust 窗口管理器（`WebviewWindowBuilder` 动态创建窗口）
4. 在 `src-tauri/capabilities/<name>.json` 中添加 capabilities 文件
5. 在 `src-tauri/tauri.conf.json` → `app.security.capabilities` 中注册 capability 名称

> ⚠️ 多数窗口在 Rust 端通过 `WebviewWindowBuilder` **动态创建**，不会出现在 `tauri.conf.json` 的 `app.windows` 数组中（只有 `main` 窗口例外 — 因其需要透明、无边框、总在最上等特殊属性）。Multi-instance 窗口（`text-editor-*`, `transfer-shelf-*`）在 capabilities 中使用通配符匹配。

## 前后端通信

```js
// 每个后端命令对应一个 async 函数（定义于 src/shared/api/）
import { invoke } from '@tauri-apps/api/core';

export async function getClipboardHistory(params) {
    try {
        return await invoke('get_clipboard_history', { params });
    } catch (error) {
        console.error('Failed to get clipboard history:', error);
        return { items: [], total: 0 };
    }
}
```

- **API 封装层**: `src/shared/api/` — 每个后端命令对应一个模块文件。完整列表: `lsp symbols file:"src/shared/api"`
- **调用**: `invoke('snake_case_command', { camelCaseArgs })` — Tauri 自动映射参数名
- **事件监听**: `listen('event-name', callback)` 订阅 Rust 端事件
- **事件发射**: `emit('event-name', payload)` 发送给后端

## 状态管理 — Valtio（唯一方案，禁止 Redux/Zustand/MobX）

```js
import { proxy, useSnapshot } from 'valtio';

// Store — 状态和方法在同一个 proxy 上
export const settingsStore = proxy({
    language: 'zh-CN',
    theme: 'auto',
    // ... 展开 defaultSettings 的所有配置项 + store 自有 UI 字段
    async setLanguage(lang) {
        settingsStore.language = lang;
        await saveSettings({ language: lang });
        i18n.changeLanguage(lang);
    },
});

// 组件中 — useSnapshot 自动追踪使用的字段，仅变化时重渲染
function MyComponent() {
    const { language } = useSnapshot(settingsStore);
}
```

Stores 完整列表: `lsp symbols file:"src/shared/store"`。详见 `src/shared/AGENTS.md`。

## i18n（强制双 locale 同步）

```jsx
import { useTranslation } from 'react-i18next';
const { t } = useTranslation();
<span>{t('settings.shortcuts.tabs.globalHotkey')}</span>
```

- 初始化: `src/shared/i18n.js` — `i18next` + `react-i18next`，fallback `zh-CN`
- **任何新 key 必须在 `zh-CN.json` 和 `en-US.json` 中同步添加** — 硬性约束
- key 格式: `dot.separated.snake_like`

## DOMPurify（所有用户 HTML 必须先净化）

```js
import { sanitizeHTML } from '@shared/utils/htmlProcessor';
// ALLOWED_TAGS/ALLOWED_ATTR 严格限制：禁止 script, iframe, object, embed, event handler
const clean = sanitizeHTML(untrustedHTML);
```

## 组件规范

- **纯函数组件** — 无 class components
- **`lazy()` + `Suspense`** — 代码分割大 tab（如 emoji picker）
- **`useCallback`/`useMemo`** — 优化高频更新子组件
- **`useSnapshot(store)`** — Valtio 状态订阅
- **`useEffect` + `listen()`** — 后端事件监听与清理（记得 return cleanup）
- **Tabler Icons**: `<i className="ti ti-check" />` via `@tabler/icons-webfont`
- **`useTranslation()`** — 所有用户可见文本必须通过 i18n

## 样式 — UnoCSS global 模式（禁用 CSS Modules）

```jsx
// 仅 utility classes
<div className="flex items-center gap-2 p-4 bg-theme-surface-1 text-theme-fg-primary">
  <button className="btn btn-primary">保存</button>
</div>
```

- **自定义 shortcuts**（定义于 `uno.config.js` — 通过 LSP 或直接读文件获取最新列表）
- **主题**: CSS 变量 token（`--qc-fg`, `--qc-surface`, `--qc-hover` 等），通过 body 的 `theme-dark`/`theme-light`/`theme-background` class 切换
- **动态主题规则**: UnoCSS 自定义规则 `bg-theme-{type}-{step}` / `text-theme-{type}-{step}` / `border-theme-{type}-{step}`
- **全局 CSS**: `src/shared/styles/` — 完整列表: `lsp symbols file:"src/shared/styles"`

## Vite alias

- `@` → `src/`
- `@shared` → `src/shared/`
- `@windows` → `src/windows/`

## 关键入口

| 文件 | 作用 |
|---|---|
| `src/windows/main/index.jsx` | 前端入口 — initStores, React root, 数据初始化, event listeners, 导航订阅 |
| `src/shared/i18n.js` | i18next 初始化 + locale 加载（fallback zh-CN） |
| `src/shared/store/settingsStore.js` | 设置状态 + load/save/update 方法 |
| `src/shared/store/clipboardStore.js` | 剪贴板 + 虚拟列表缓存 |

## LSP 实时查询

| 想了解 | LSP 命令 |
|---|---|
| 所有窗口目录 | `lsp symbols file:"src/windows"` |
| 所有 API 模块 | `lsp symbols file:"src/shared/api"` |
| 所有 hooks | `lsp symbols file:"src/shared/hooks"` |
| 所有 Valtio stores | `lsp symbols file:"src/shared/store"` |
| 所有 UI 组件 | `lsp symbols file:"src/shared/components"` |
| 组件引用查找 | `lsp references file:"<path>" symbol:"<Component>"` |

## 反模式

- ❌ 创建 `.ts`/`.tsx` 文件
- ❌ 使用 Redux / Zustand / MobX / Jotai 代替 Valtio
- ❌ 使用 CSS Modules / styled-components（用 UnoCSS utility classes）
- ❌ 只在单一 locale 文件添加 i18n key
- ❌ 渲染用户 HTML 不经 DOMPurify
- ❌ 使用 React Router（导航用 state，非 URL）
- ❌ 在组件卸载时忘记清理 listener
