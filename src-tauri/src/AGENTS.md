# src-tauri/src/ — Rust 后端核心

Tauri 2 应用的 Rust 代码入口。`lib.rs` 定义 `quickclipboard_lib` crate，`main.rs` 调用 `run()`。

> **实时查询：** 完整模块列表的权威来源是 `commands/mod.rs`、`services/mod.rs`、`windows/mod.rs` 中的 `pub mod` 声明。也可通过 LSP 实时获取：`lsp symbols` 在对应目录上即可列出当前所有被编译的模块。

## 结构

```
src/
├── lib.rs            # crate root，模块声明 + pub use 导出
├── main.rs           # 二进制入口，调用 lib::run() 或维护模式
├── startup_diagnostics.rs  # 启动诊断（panic hook、旧进程检测、状态文件）
├── commands/         # Tauri invoke 命令模块（权威来源: commands/mod.rs）
│   ├── clipboard.rs  #   剪贴板 CRUD、粘贴（代表性）
│   ├── settings.rs   #   设置保存/加载、快捷键
│   └── ...           #   其余模块通过 LSP 或 mod.rs 查看
├── services/         # 业务逻辑层（无 Tauri 依赖，权威来源: services/mod.rs）
│   ├── clipboard/    #   monitor（SHA256 去重 + atomic pause/suppress）
│   │                 #   capture（多格式：text/RTF/HTML/files/image）
│   │                 #   processor（内容分类 + 来源应用识别 + HTML 规范化）
│   ├── database/     #   SQLite 单连接（Mutex + Lazy），WAL 模式
│   │                 #   models: ClipboardItem, FavoriteItem, GroupInfo 等
│   ├── settings/     #   model.rs: AppSettings, storage.rs: 文件持久化
│   ├── paste/        #   paste_handler（enigo 键盘模拟）、merge（多条目合并）
│   ├── system/       #   hotkey（Tauri global shortcut）、raw_input、startup
│   ├── webdav_sync/  #   client, uploader, downloader, crypto（XChaCha20Poly1305+Argon2id）
│   │                 #   scheduler, cloud files, groups/tombstones sync
│   ├── sync_transfer/ #  LAN P2P: HTTP server/client, UDP discovery, pairing
│   └── ...           #   low_memory, notification, memory, secure_credentials 等
├── windows/          # 窗口生命周期管理（权威来源: windows/mod.rs）
│   ├── main_window/  #   管理器、可见性、边缘吸附/动画
│   ├── quickpaste/   #   快速粘贴窗口
│   ├── tray/         #   系统托盘
│   └── ...           #   其余窗口通过 LSP 或 mod.rs 查看
├── security/         # WebView2 env var 安全审计（release 构建时拦截危险参数）
├── maintenance/      # 独立 TUI 维护模式（ratatui + crossterm）
└── utils/            # screen, positioning, icon, text, html/cf_html, image, mouse
```

实际的模块注册以 `commands/mod.rs`、`services/mod.rs`、`windows/mod.rs` 中的 `pub mod` 声明为准；目录中存在但未在 `mod.rs` 中声明的子目录不会被编译。注意 `state/`、`media/`、`ai/`、`file/`、`ui/`、`image/` 等目录存在于磁盘但未被编译 — 它们是非活跃目录，不应使用。

## 架构决策

### 错误处理

不使用 `anyhow` 或 `thiserror`。`#[tauri::command]` 返回 `Result<T, String>`，`String` 即错误类型。内部函数同样返回 `Result<T, String>`，通过 `.map_err(|e| format!(...))` 转换。

```rust
#[tauri::command]
fn my_command() -> Result<MyData, String> {
    do_work().map_err(|e| format!("操作失败: {}", e))
}
```

### 数据库访问

```rust
// 单连接 + Mutex + Lazy，通过闭包访问
with_connection(|conn| {
    conn.execute("INSERT INTO clipboard (...) VALUES (...)", params![...])?;
    Ok(())
})?; // 外层将 rusqlite::Error 转为 String
```

Pragma: `WAL`, `foreign_keys = ON`, `synchronous = NORMAL`, `cache_size = 10000`, `temp_store = MEMORY`。无 ORM — raw SQL + `rusqlite::params![]`。Migration 通过 `PRAGMA table_info` / `PRAGMA user_version` 实现。

### 全局状态

```rust
use once_cell::sync::Lazy;
use parking_lot::Mutex;

static DB_CONNECTION: Lazy<Mutex<Option<Connection>>> = Lazy::new(|| Mutex::new(None));
static SETTINGS: Lazy<RwLock<AppSettings>> = Lazy::new(|| RwLock::new(AppSettings::default()));
```

不使用 Tauri 的 `app.manage()`。所有全局状态是模块级 `Lazy<Mutex<T>>` 或 `Lazy<RwLock<T>>`。关键静态变量：`DB_CONNECTION`, `SETTINGS`, `MONITOR_STATE`, `LAST_CONTENT_HASHES`, `APP_HANDLE`。原子标志：`IS_RUNNING`, `GENERATION`, `MONITOR_PAUSE_COUNT`, `CAPTURE_IN_FLIGHT`。

### Async 模式

```rust
#[tauri::command]
async fn heavy_command(...) -> Result<T, String> {
    tokio::task::spawn_blocking(move || {
        // CPU/IO 密集型工作：SQLite、OCR、文件操作、图片处理
    })
    .await
    .map_err(|e| format!("Join error: {}", e))?
}
```

- Tokio runtime（multi-thread）用于后台任务
- `spawn_blocking` 处理同步阻塞操作
- `tauri::async_runtime::spawn` 用于 fire-and-forget 后台启动
- Clipboard monitor 使用 `std::thread`（非 tokio）

### 线程模型

| 线程 | 用途 |
|---|---|
| Main thread | Tauri event loop, non-blocking commands |
| Clipboard monitor | `std::thread` — 长期阻塞的 `clipboard-rs` 监听 |
| tokio runtime | 后台任务（LAN sync server, WebDAV scheduler, crypto） |
| `spawn_blocking` | SQLite、OCR、文件 I/O |

### Serde 约定

```rust
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]    // JS 互通
pub struct AppSettings {
    #[serde(skip_serializing)]         // 密钥不序列化
    pub webdav_password: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optional_field: Option<String>,
}
```

## LSP 实时查询

| 想了解 | LSP 命令 |
|---|---|
| 所有 command 模块 | `lsp symbols file: "src-tauri/src/commands"` |
| 所有 service 模块 | `lsp symbols file: "src-tauri/src/services"` |
| 所有 window 模块 | `lsp symbols file: "src-tauri/src/windows"` |
| generate_handler 注册了哪些命令 | `lsp references file:"src-tauri/src/lib.rs" symbol:"generate_handler"` |
| 某个函数的调用者 | `lsp references file:"<path>" symbol:"<func>"` |
| 某个 trait 的实现者 | `lsp implementation file:"<path>" symbol:"<trait>"` |

## Feature gate

私有插件（截屏 `screenshot-suite`、GPU 贴图 `gpu-image-viewer`）已于 2026-08 剥离。`Cargo.toml` 仅保留 `default = []` 与 `custom-protocol` feature。

Windows-only 依赖按平台声明：
- `qcocr`（OCR）、`winreg` 位于 `[target.'cfg(windows)'.dependencies]`
- OCR 相关代码 `#[cfg(windows)]` 门控：`commands/ocr.rs`（模块级）、`services/image_library/mod.rs` 的 `ocr_image_text` 与其在 `save_image` 中的调用点

## 新增 Tauri 命令

1. **创建命令模块** — `commands/<name>.rs`，实现 `#[tauri::command]` 函数
2. **注册模块** — `commands/mod.rs` 中 `pub mod <name>` + `pub use <name>::*`
3. **添加到 handler** — `lib.rs` 的 `generate_handler![]` 列表中
4. **声明权限** — 添加到对应的 `capabilities/*.json`
5. **创建前端封装** — `src/shared/api/<name>.js` 中包装 `invoke()`

遗漏任一步 → 前端 `invoke()` 静默失败（无编译错误）。

## 关键入口

| 文件 | 作用 |
|---|---|
| `lib.rs` | Crate root — `run()`: 注册 plugins, 注册所有 commands, `setup()` 初始化所有子系统 |
| `main.rs` | Binary entry — `--maintenance` 进入维护模式，否则 `lib::run()` |
| `startup_diagnostics.rs` | Panic hook（含启动阶段追踪）、上次实例检测（状态文件 + PID） |
| `security/mod.rs` | WebView2 env var 安全审计（release 构建拦截 `--remote-debugging-port` 等） |
| `build.rs` | `tauri_build::build()` |

## SQLite 约束

- 单连接 + `parking_lot::Mutex`，WAL 模式
- **不支持并发写入** — 避免多线程同时写库
- 连接获取：`services/database/connection.rs` 中的 `with_connection()` 闭包模式
- 访问模式：`with_connection(|conn| { ... })` 获取 `&Connection`，返回 `Result<R, rusqlite::Error>`

## 反模式

- ❌ 在 `lib.rs` 中直接引用私有插件类型（已剥离，禁止重新引入）
- ❌ 跳过 capabilities 权限声明
- ❌ 并发写入 SQLite
- ❌ 使用 `crate::` 导入内部模块（优先 `super::super::...`，路径深时才用 `crate::`）
- ❌ 在 services 层引入 Tauri 依赖（services 应纯业务逻辑）
- ❌ 在 `#[tauri::command]` 中直接执行长时间同步操作（应通过 `spawn_blocking`）
- ❌ 使用 `state/`、`media/`、`ai/` 等非活跃目录（未被 mod.rs 编译，功能不会生效）
