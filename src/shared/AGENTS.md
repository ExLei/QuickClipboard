# src/shared/ — 共享模块

跨窗口复用的前端代码。本项目所有共享逻辑在此。

> **实时查询：** 完整模块列表通过 LSP 获取权威数据。下文树结构仅展示代表性模块，各子目录的具体文件列表通过 `lsp symbols` 实时查询。

## 结构

```
shared/
├── api/              # 前后端通信封装（invoke 包装器，每个后端命令一个文件）
│   └── ...           #   完整列表: lsp symbols file:"src/shared/api"
├── components/       # 共享 UI 组件
│   ├── ui/           #   基础 UI（完整列表: lsp symbols）
│   └── common/       #   通用组件（完整列表: lsp symbols）
├── config/           # 全局配置常量
├── constants/        # 常量定义（tabVisibility 等）
├── hooks/            # 共享 React hooks — 完整列表: lsp symbols file:"src/shared/hooks"
├── i18n.js           # i18next 初始化（fallback: zh-CN）
├── locales/          # zh-CN.json + en-US.json（两个文件必须同步）
├── services/         # 前端业务逻辑（eventListener, settingsService）
├── store/            # Valtio proxy 状态 — 完整列表: lsp symbols file:"src/shared/store"
├── styles/           # 全局 CSS（主题变量、暗色模式、滚动条、动画、HTML 内容）
└── utils/            # 工具函数（htmlProcessor 等）
```

## 前后端通信 (API 层)

```js
// src/shared/api/clipboard.js — 标准封装模式
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

- **命名映射**: JS `camelCase` 函数 → Rust `snake_case` 命令
- **参数**: JS object 自动映射，Tauri 处理 camelCase ↔ snake_case 转换
- **错误处理**: try/catch 包装 + 错误日志 + 失败默认值
- **模块列表**: `lsp symbols file:"src/shared/api"`

## i18n 协议（强制）

```jsx
import { useTranslation } from 'react-i18next';
const { t } = useTranslation();
<span>{t('settings.shortcuts.tabs.globalHotkey')}</span>
```

- **初始化**: `src/shared/i18n.js` — `i18next` + `react-i18next`，fallback `zh-CN`
- **语言切换**: `settingsStore.setLanguage()` → `i18n.changeLanguage()`
- **key 格式**: `dot.separated.snake_like`（如 `settings.shortcuts.tabs.globalHotkey`）
- **新增文本**: 同时在 `locales/zh-CN.json` 和 `locales/en-US.json` 中添加
- **禁止**: 只在单一 locale 中添加 key

## Valtio store（唯一状态管理方案）

```js
// store/settingsStore.js — 标准 pattern
import { proxy } from 'valtio';

export const settingsStore = proxy({
    // 状态字段（展开 defaultSettings 的所有配置项 + UI 专属字段）
    language: 'zh-CN',
    theme: 'auto',
    // ...

    // 方法直接挂在 proxy 上
    async loadSettings() { /* 从后端加载 */ },
    async saveSetting(key, value) { /* 保存单个设置 */ },
    async saveSettings(settings) { /* 批量保存 */ },
});
```

```jsx
// 组件中使用
import { useSnapshot } from 'valtio';
import { settingsStore } from '@shared/store/settingsStore';

function MyComponent() {
    const { theme } = useSnapshot(settingsStore);
    // 仅当 theme 变化时重渲染
}
```

禁止 Redux / Zustand / MobX / Jotai。Stores 完整列表及职责: `lsp symbols file:"src/shared/store"`。

## 事件监听服务

```js
// src/shared/services/eventListener.js — 标准监听模式
import { listen } from '@tauri-apps/api/event';

const unlisteners = [];

export async function setupListeners() {
    const unlisten = await listen('clipboard-updated', (event) => {
        clipboardStore.insertItemAtTop(event.payload);
    });
    unlisteners.push(unlisten);
    // ... 更多 listener 注册
}

export function cleanupListeners() {
    unlisteners.forEach(fn => fn());
    unlisteners.length = 0;
}
```

## DOMPurify（安全强制）

```js
import { sanitizeHTML } from '@shared/utils/htmlProcessor';
// ALLOWED_TAGS/ALLOWED_ATTR 严格限制：
// 禁止 script, iframe, object, embed, event handler 属性
const clean = sanitizeHTML(untrustedHTML);
```

**所有用户 HTML 在渲染前必须经过此函数。**

## Import alias

- `@shared/*` → `src/shared/*`
- 窗口代码可 import 此目录下的模块
- Vite alias 同时支持 `@` → `src/`、`@windows` → `src/windows/`

## LSP 实时查询

| 想了解 | LSP 命令 |
|---|---|
| API 模块列表 | `lsp symbols file:"src/shared/api"` |
| Hooks 列表 | `lsp symbols file:"src/shared/hooks"` |
| Stores 列表 | `lsp symbols file:"src/shared/store"` |
| UI 组件列表 | `lsp symbols file:"src/shared/components"` |
| 某个函数的调用者 | `lsp references file:"<path>" symbol:"<func>"` |
| 某个符号的定义 | `lsp definition file:"<path>" symbol:"<name>"` |

## 反模式

- ❌ 在此处使用 TypeScript（`.ts`/`.tsx`）
- ❌ 使用 Redux / Zustand / MobX / Jotai 替代 Valtio
- ❌ 在 i18n 中只更新单一 locale
- ❌ 直接渲染未净化的 HTML（必须用 `sanitizeHTML()`）
- ❌ 在 API 封装中吞掉错误而不打日志
- ❌ 忘记在组件卸载时清理 listener（`useEffect` return cleanup）
