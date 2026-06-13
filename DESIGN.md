# BiliTools CLI 架构设计

> 基于 `ANALYSIS.md` 的映射表，给出 CLI 的完整设计。

---

## 1. Crate 拓扑

```
bilicli/                       # binary crate, name = "bilicli"
├── Cargo.toml
├── src/
│   ├── main.rs                       # 入口
│   ├── lib.rs                        # 暴露给 integration test 的公共 API
│   ├── context.rs                    # AppContext
│   ├── error.rs                      # CliError + Result 别名
│   ├── doctor.rs                     # 健康检查
│   ├── backends/                     # 替代 Tauri 的适配层
│   │   ├── mod.rs
│   │   ├── http.rs                   # reqwest + proxy
│   │   ├── sidecar.rs                # aria2/ffmpeg/DanmakuFactory 子进程
│   │   └── paths.rs                  # XDG/AppData 路径
│   ├── ipc/                          # 移植自 BiliTools 业务代码（去 Tauri 化）
│   │   ├── mod.rs
│   │   ├── shared.rs                 # HEADERS, init_client, USER_AGENT, get_*
│   │   ├── login.rs                  # 扫码登录
│   │   ├── aria2c.rs                 # Aria2 RPC 客户端
│   │   ├── ffmpeg.rs                 # FFmpeg 调用
│   │   ├── media.rs                  # URL 解析 → ResourceDescriptor
│   │   ├── bilibili_api.rs           # B 站高层 API（view/fav/season）
│   │   ├── storage/
│   │   │   ├── mod.rs
│   │   │   ├── config.rs             # Settings + 字段裁剪
│   │   │   ├── db.rs
│   │   │   ├── cookies.rs
│   │   │   ├── queue.rs
│   │   │   ├── schedulers.rs
│   │   │   ├── tasks.rs
│   │   │   └── migrate.rs
│   │   └── queue/
│   │       ├── mod.rs
│   │       ├── atomics.rs
│   │       ├── types.rs
│   │       ├── task.rs
│   │       ├── scheduler.rs
│   │       ├── manager.rs
│   │       ├── runtime.rs
│   │       ├── handlers.rs
│   │       └── frontend.rs
│   ├── cli/
│   │   ├── mod.rs
│   │   ├── root.rs                   # clap::Parser 根
│   │   ├── output.rs                 # --json / 人类可读统一输出
│   │   ├── auth.rs                   # auth 子命令
│   │   ├── parse.rs                  # parse 子命令
│   │   ├── download.rs               # download 子命令
│   │   ├── schedule.rs               # schedule 子命令
│   │   ├── config.rs                 # config 子命令
│   │   ├── cache.rs                  # cache 子命令
│   │   ├── db.rs                     # db 子命令
│   │   ├── info.rs                   # info 子命令
│   │   └── repl.rs                   # REPL
│   └── tools/
│       └── url.rs                    # B 站 URL 解析器
└── tests/
    ├── unit/
    ├── integration/
    └── e2e/
```

## 2. 依赖选型

**直接沿用 BiliTools 的依赖 + 增删：**

| 类别 | BiliTools | bilicli | 理由 |
|---|---|---|---|
| 异步 | `tokio 1.48` | `tokio 1` | 同 |
| HTTP | `tauri_plugin_http::reqwest` | `reqwest 0.12` | 解耦 tauri |
| DB | `sqlx 0.8` (sqlite + runtime-tokio) | 同 | schema 沿用 |
| 序列化 | `serde`, `serde_json` | 同 + `serde_yaml` | config 显示 |
| 时间 | `time 0.3` | 同 | 沿用 |
| 加密 | `hmac 0.12`, `sha2 0.10` | 同 | B 站 WBI/buvid |
| 随机 | `rand 0.9` | 同 | uuid 生成 |
| 日志 | `tauri_plugin_log` | `tracing 0.1` + `tracing-subscriber` + `tracing-appender` | 解耦 tauri |
| CLI 框架 | (无) | `clap 4` (derive) | 新增 |
| REPL | (无) | `rustyline 14` | 新增（轻量、稳定） |
| 路径 | `tauri::Manager::path` | `directories 5` | 解耦 tauri |
| 错误 | `anyhow 1.0` | `anyhow` + `thiserror 1` | 同时支持 |
| specta | `specta 2.0-rc22` | **删除** | CLI 不用 TS 导出 |
| tauri | `tauri 2.9.5` | **删除** | 整项目解耦 |
| **新增** | | `wiremock 0.6` | integration test |
| **新增** | | `tempfile 3` | 测试用 tmp dir |
| **新增** | | `assert_cmd 2` | CLI 集成测试 |
| **新增** | | `predicates 3` | assert_cmd 配套 |
| **新增** | | `tokio-test 0.4` | 异步断言 |

**移除的 tauri 全部插件：**
- tauri-plugin-clipboard-manager, dialog, http, log, notification, opener, os, process, shell, single-instance, updater

## 3. CLI 命令树（clap）

```
bilicli [OPTIONS] [COMMAND]

OPTIONS:
    -j, --json              # 机器可读 JSON 输出（所有子命令）
    -c, --config <PATH>     # 配置文件路径
        --data-dir <PATH>   # 数据目录覆盖
        --log-level <LVL>   # trace|debug|info|warn|error
        --no-color          # 关闭 ANSI 颜色
    -h, --help
    -V, --version
    --doctor                # 启动前健康检查

COMMANDS:
    info                    # 版本/路径/构建信息
    init                    # 初始化环境（buvid3/buvid4/ticket/uuid）
    auth                    # 认证
    ├── qrcode              # 启动扫码，写 PNG 到 --output
    ├── qrcode-poll <key>   # 单次轮询扫码状态
    ├── qrcode-cancel       # 取消扫码
    ├── status              # 当前登录态
    ├── refresh             # 手动刷新 cookie
    └── exit                # 登出
    parse                   # 解析（不下载）
    ├── url <URL>           # 任意 B 站 URL
    ├── bv <BV_ID>
    ├── av <AV_ID>
    ├── fav <FID>           # 收藏夹
    ├── watchlater
    ├── bangumi <SS_ID>
    ├── episode <EP_ID>
    └── interactive <BV_ID> # 互动视频
    download                # 下载
    ├── submit <RESOURCE>   # 提交单个
    ├── batch <FILE>        # 批量（行分隔 URL）
    ├── list [--status]     # 列出所有任务
    ├── show <TASK_ID>      # 任务详情
    ├── cancel <TASK_ID>
    ├── pause <TASK_ID>
    ├── resume <TASK_ID>
    ├── retry <TASK_ID>
    └── open <TASK_ID>      # 打开下载目录
    schedule                # 计划任务
    ├── list
    ├── add <CRON> <RESOURCE>
    ├── remove <SCHED_ID>
    └── run <SCHED_ID>      # 立即触发
    config                  # 配置
    ├── show
    ├── get <KEY>
    ├── set <KEY> <VALUE>
    ├── reset
    └── path
    cache                   # 缓存管理
    ├── list
    ├── size
    ├── clean
    └── open
    db                      # 数据库
    ├── export <FILE>
    ├── import <FILE>
    └── tasks
    repl                    # 交互式 REPL（默认无子命令时也进）
    doctor                  # 独立健康检查
```

## 4. 状态模型

### 4.1 进程内（每次调用）

```rust
pub struct AppContext {
    pub data_dir: PathBuf,        // XDG_DATA_HOME/com.nekobaimeow.bilicli
    pub log_dir: PathBuf,
    pub temp_dir: PathBuf,
    pub config_path: PathBuf,
    pub db_path: PathBuf,
    pub config: Arc<RwLock<Settings>>,
    pub http: reqwest::Client,    // 共享 client
    pub http_no_proxy: reqwest::Client,
    pub event_tx: mpsc::Sender<UiEvent>,  // 进度/日志事件（REPL/--json 模式用）
}
```

### 4.2 持久化（SQLite）

沿用 BiliTools 全部 schema：
- `cookies`, `tasks`, `queue`, `schedulers`, `settings`（JSON blob）

CLI 数据库路径与 GUI 一致（Linux: `$XDG_DATA_HOME/com.nekobaimeow.bilicli/Storage/storage.db`），支持**与 GUI 版数据互通**。

### 4.3 跨进程

- **Cookie**：通过 SQLite 共享
- **任务状态**：通过 SQLite + 文件锁（`postring` 简单加锁）
- **配置**：SQLite 单写多读

## 5. 统一输出（`Output`）

```rust
pub struct Output {
    pub mode: OutputMode,   // Human | Json
    pub writer: Box<dyn Write>,
    pub no_color: bool,
}

pub enum OutputMode { Human, Json }

impl Output {
    pub fn ok<T: Serialize>(&self, data: T) -> io::Result<()> { ... }
    pub fn err(&self, err: &CliError) -> io::Result<()> { ... }
    pub fn table<T: Serialize>(&self, rows: Vec<T>) -> io::Result<()> { ... }  // Human 模式用 comfy_table
    pub fn event(&self, ev: UiEvent) -> io::Result<()> { ... }  // REPL 用
}
```

**JSON Schema（统一）：**
```json
{
  "ok": true,
  "data": { ... },        // 命令特定数据
  "meta": {                // 可选
    "duration_ms": 1234
  }
}
```

错误格式：
```json
{
  "ok": false,
  "error": {
    "code": "AUTH_REQUIRED",
    "message": "未登录，请先运行 `bilicli auth qrcode`",
    "context": { "hint": "..." }
  }
}
```

## 6. REPL 设计

```
$ bilicli repl
bilicli v1.4.7-cli (rev: 0a19072)
Type 'help' for commands. Ctrl-D or 'exit' to quit.

bilicli> auth status
{
  "logged_in": false,
  "user": null,
  "cookies": ["buvid3", "buvid4", "_uuid", "bili_ticket"]
}

bilicli> config set max_conc 4
OK

bilicli> download submit "https://www.bilibili.com/video/BV1xx411c7mD"
{
  "task_id": "abc123",
  "status": "queued",
  "type": "AudioVideo"
}

bilicli> exit
$
```

**实现要点：**
- 用 `rustyline` 提供行编辑 + 历史
- 历史持久化到 `$XDG_DATA_HOME/bilicli/repl_history`
- 子命令解析：直接 `clap::Command::try_get_matches_from_mut`
- 错误用红字，成功用绿字（`--no-color` 关）
- 长任务用 `>>>` 提示行尾显示 spinner

## 7. 配置模型

CLI 专属 `Settings`（去 GUI 字段后）：

```rust
pub struct Settings {
    pub add_metadata: bool,           // 保留
    pub auto_download: bool,          // 保留（CLI 也用）
    pub block_pcdn: bool,             // 保留
    pub clipboard: bool,              // 砍（GUI）
    pub convert: SettingsConvert,
    pub default: SettingsDefault,
    pub down_dir: PathBuf,
    pub drag_search: bool,            // 砍
    pub format: SettingsFormat,
    pub language: String,             // 保留（CLI 错误信息 i18n）
    pub max_conc: usize,
    pub notify: bool,                 // 砍
    pub temp_dir: PathBuf,
    pub theme: Theme,                 // 砍 → 移到独立 ui_prefs
    pub window_effect: WindowEffect,  // 砍
    pub organize: SettingsOrganize,
    pub proxy: SettingsProxy,
    pub sidecar: SettingsSidecar,     // 改成可执行文件路径
    pub speed_limit: serde_json::Number,
}

pub struct SettingsSidecar {
    pub aria2c: PathBuf,              // 改：默认 `which aria2c` 路径
    pub ffmpeg: PathBuf,
    pub danmakufactory: PathBuf,      // 用户自配
    pub block_pcdn: bool,
}
```

**Config get/set 路径用点号：** `bilicli config set sidecar.ffmpeg /usr/local/bin/ffmpeg`

## 8. 错误类型

```rust
#[derive(thiserror::Error, Debug)]
pub enum CliError {
    #[error("config: {0}")]
    Config(String),

    #[error("auth: {0}")]
    Auth(#[from] AuthError),

    #[error("network: {0}")]
    Network(#[from] reqwest::Error),

    #[error("database: {0}")]
    Database(#[from] sqlx::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("bilibili api error: code={code} {message}")]
    Api { code: i64, message: String },

    #[error("not logged in: {0}")]
    NotLoggedIn(String),

    #[error("invalid url: {0}")]
    InvalidUrl(String),

    #[error("dependency missing: {0}")]
    MissingDependency(String),  // aria2c/ffmpeg/DanmakuFactory

    #[error("{0}")]
    Other(String),
}

pub type CliResult<T> = Result<T, CliError>;
```

## 9. 关键设计决策

| 决策 | 选 A | 选 B | 选 C | 决定 |
|---|---|---|---|---|
| CLI 框架 | clap v4 (derive) | argh | structopt | **clap** (生态最大) |
| REPL 库 | rustyline | clap-repl | reedline | **rustyline** (稳) |
| HTTP | reqwest 0.12 | ureq | surf | **reqwest** (与原兼容) |
| 日志 | tracing | log 0.4 | slog | **tracing** (现代) |
| DB 驱动 | sqlx (沿用) | diesel | rusqlite | **sqlx** (沿用 schema) |
| 进度条 | indicatif | 手动 ANSI | 无 | **indicatif**（仅人类模式） |
| 表格 | comfy-table | term-table | 手动 | **comfy-table**（仅人类模式） |
| 错误打印 | thiserror + anyhow | 只 anyhow | 只 thiserror | **两者**（thiserror 公开，anyhow 内部） |

## 10. 与 GUI 版数据兼容性

| 路径 | 共享 | 备注 |
|---|---|---|
| SQLite DB | ✅ 同路径 | CLI 写 = GUI 读 |
| 下载目录 | ✅ 同路径 | `down_dir` 配置一致 |
| 临时文件 | ✅ 同路径 | `temp_dir` 配置一致 |
| 日志 | ⚠️ 独立 | CLI 写到 `$XDG_DATA_HOME/bilicli/logs/`，GUI 写到 app_log_dir |
| 配置 | ✅ 共享 | 都从 SQLite `settings` 表读 |

## 11. 入口行为

```
bilicli                  # 默认 → repl
bilicli <subcommand>     # 执行子命令
bilicli --version        # 版本
bilicli --help           # clap 自动 help
```

信号处理：SIGINT 触发优雅退出（取消当前 task，关闭 aria2 子进程）。

## 12. 测试策略

- **Unit**: 工具函数、URL 解析、config get/set、headers 构造
- **Integration**: 用 `wiremock` 模拟 B 站 API，跑完整子命令链路
- **E2E**: 真实 B 站 API（标 `#[ignore]`，手工跑）

---

## 验收

✅ 命令树完整，无歧义
✅ 状态模型三层（进程内/持久化/跨进程）清晰
✅ 统一输出层 `Output` + JSON Schema 已定
✅ REPL 设计明确（rustyline）
✅ Tauri 解耦方案逐项对应 ANALYSIS.md
✅ 错误类型 `CliError` 覆盖所有失败模式
✅ 测试策略三层（unit/integration/E2E）
