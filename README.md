# bilitools — A pure-Rust CLI port of [BiliTools](https://github.com/btjawa/BiliTools)

> 简体中文 | [English](#english)

`bilitools` 是一个 100% Rust 实现的命令行工具，复刻了 BiliTools（一个跨平台 Bilibili 工具箱）的功能。
**我们直接复用了 BiliTools 5,464 行 Rust 业务代码**（登录 / aria2c / ffmpeg / 队列 / 解析 / 数据库），
只是把 Tauri GUI 框架剥掉，改成 clap 命令行界面。

这意味着：
- ✅ 相同的 B 站 API 集成（WBI 签名、Buvid 指纹、扫码登录、Cookie 持久化）
- ✅ 相同的 aria2c RPC 下载后端
- ✅ 相同的 ffmpeg 媒体后处理
- ✅ 相同的 SQLite 任务队列 — **与 GUI 版数据库互通**
- ✅ 同样的 GPL-3.0-or-later 协议（上游强制）
- ❌ 没有 Tauri 窗口、主题、剪贴板监听、单实例

## 快速开始

### 1. 编译安装

```bash
git clone https://github.com/btjawa/BiliTools
cd bilitools-cli
cargo install --path .
```

需要 Rust 1.80+。

### 2. 安装系统依赖

`bilitools` 复用三个外部命令（GUI 版用 Tauri 打包 sidecar；CLI 版用系统包）：

```bash
# Debian / Ubuntu
sudo apt install aria2 ffmpeg

# macOS
brew install aria2 ffmpeg

# Arch
sudo pacman -S aria2 ffmpeg
```

可选：DanmakuFactory（弹幕 XML→ASS 转换）：
- 下载：<https://github.com/hihkm/DanmakuFactory/releases>
- 放到 `PATH` 里的某个位置

### 3. 验证

```bash
bilitools --version
bilitools doctor
```

`doctor` 会检查 aria2/ffmpeg/DanmakuFactory 是否就绪 + B 站 API 是否可达。

### 4. 登录

扫码登录（推荐，B 站风控最稳定）：

```bash
# 生成二维码 + 写到 ./qr.png + 输出 qrcode_key
bilitools auth qrcode --output ./qr.png --json

# 用手机 B 站扫一下
xdg-open ./qr.png  # 或在手机上扫

# 然后轮询登录状态（可以用 QR-key 在 REPL 里持续轮询）
bilitools auth qrcode-poll <qrcode_key>
```

> CLI 不会自动把二维码弹窗。你需要自己用手机扫 PNG/URL。

### 5. 解析 / 下载

```bash
# 解析一个 BV 号（不发请求，纯 URL 分类）
bilitools parse bv BV1xx411c7mD

# 解析一个番剧 SS 号
bilitools parse url "https://www.bilibili.com/bangumi/play/ss28280"

# 提交下载（写入数据库 + 拉取 view 元数据）
bilitools download submit "https://www.bilibili.com/video/BV1xx411c7mD" --quality 80 --output-dir ~/Videos

# 真下载 — 启动 aria2c RPC daemon + 下载 DASH 段 + ffmpeg 合并 mp4
bilitools download run <task_id>

# 列出所有任务
bilitools download list

# 取消
bilitools download cancel <task_id>

# 重试失败 / 已完成（会先看磁盘文件，aria2c 自动续传）
bilitools download retry <task_id>
```

**断点续传**：aria2c 启动时带 `--continue=true --check-integrity=true --max-tries=0`。
如果某个分片（视频 / 音频 m4s）下载途中被杀，再 `bilitools download run` 同一 task，
aria2c 会用 HTTP Range 请求把缺失的字节范围续上，最后 ffmpeg 合并成 mp4。

E2E 验证（BV1ZvEt6oEWR 锐评 2026 数学高考，738 秒单 P）：
- 完整下载：19 秒，~31 MB
- 中断后重下（100 KB 残片）：7 秒，`resumed: true`，ffprobe 验证可播放

### 6. 配置

```bash
bilitools config show
bilitools config get max_conc
bilitools config set max_conc 4
bilitools config set sidecar.aria2c /usr/bin/aria2c
```

### 7. REPL

```bash
bilitools repl
```

进入交互模式（`>>>` 提示符，支持历史），可以连续跑命令。

## 命令参考

```
bilitools [OPTIONS] [COMMAND]

Options:
    -j, --json                   机器可读 JSON 输出
    --config <FILE>              配置文件路径
    --data-dir <DIR>             数据目录覆盖
    --log-level <LEVEL>          trace|debug|info|warn|error
    --no-color                   关闭 ANSI 颜色
    --doctor                     启动前健康检查

Commands:
  info      版本 + 路径
  init      初始化环境（buvid3/buvid4/ticket/uuid）
  auth      认证（qrcode / qrcode-poll / status / refresh / exit）
  parse     解析（不下载）：url / bv / av / bangumi / episode / fav / watchlater / user
  download  下载：submit / batch / list / show / cancel / pause / resume / retry / open
  schedule  计划任务：list / add / remove / run
  config    show / get / set / reset / path
  cache     list / size / clean / open
  db        export / import / tasks
  doctor    健康检查
  repl      交互式 REPL
```

每个子命令支持 `--help` 看细节：`bilitools download --help`。

## JSON 输出

所有子命令支持 `--json` 标志，输出统一信封：

```json
{
  "ok": true,
  "data": { ... }
}
```

错误时：

```json
{
  "ok": false,
  "error": {
    "code": "NOT_LOGGED_IN",
    "message": "请先运行 `bilitools auth qrcode`"
  }
}
```

错误码列表（稳定，可编程处理）：`CONFIG / AUTH / NETWORK / DATABASE / IO / SERDE / API / HTTP / NOT_LOGGED_IN / INVALID_URL / MISSING_DEPENDENCY / PATH_NOT_FOUND / PARSE / TASK_NOT_FOUND / CANCELLED / OTHER`。

## 数据目录

跨平台一致：

- Linux: `$XDG_DATA_HOME/com.btjawa.bilitools/`
- macOS: `~/Library/Application Support/com.btjawa.bilitools/`
- Windows: `%AppData%\com.btjawa.bilitools\`

子目录：

```
Storage/storage.db      # SQLite 主数据库（与 GUI 版互通）
logs/                   # CLI 日志
cache/                  # 通用缓存
runtime/                # 临时 runtime 文件（如 aria2 socket）
```

可用 `BILITOOLS_DATA_DIR` 环境变量或 `--data-dir` 标志覆盖。

## 开发

```bash
# 跑全部测试
cargo test -- --test-threads=1

# 跑单个模块
cargo test --lib storage::cookies
cargo test --lib ipc::media

# 端到端测试（需要登录态，标了 #[ignore]）
cargo test --lib -- --ignored

# 实时日志
RUST_LOG=bilitools=debug cargo run -- doctor
```

## 协议

GPL-3.0-or-later — 与上游 BiliTools 一致。

## 致谢

- [btjawa/BiliTools](https://github.com/btjawa/BiliTools) — 5,464 行 Rust 业务代码
- [SocialSisterYi/bilibili-API-collect](https://github.com/SocialSisterYi/bilibili-API-collect) — B 站 API 文档
- [aria2](https://github.com/aria2/aria2) / [FFmpeg](https://ffmpeg.org/) / [DanmakuFactory](https://github.com/hihkm/DanmakuFactory) — 底层工具

---

## English

`bilitools` is a 100% Rust CLI port of BiliTools. It reuses the original Rust
business logic (5,464 LOC of login / aria2c / ffmpeg / queue / parsing / DB
code) with the Tauri GUI layer replaced by clap subcommands.

### Install

```bash
git clone https://github.com/btjawa/BiliTools
cd bilitools-cli
cargo install --path .

# Install aria2 + ffmpeg
sudo apt install aria2 ffmpeg
```

### Quick start

```bash
bilitools doctor
bilitools auth qrcode --output ./qr.png
# scan with B 站 app on your phone, then:
bilitools auth qrcode-poll <qrcode_key>
bilitools parse bv BV1xx411c7mD
bilitools download list
```

### Documentation

- `ANALYSIS.md` — GUI→API mapping and design rationale
- `DESIGN.md` — architecture, command tree, state model
- `TEST.md` — test plan and how to run
- `SKILL.md` — agent-facing description (for `npx skills add`)

### License

GPL-3.0-or-later (inherited from BiliTools).

### Acknowledgements

- **[btjawa/BiliTools](https://github.com/btjawa/BiliTools)** — the upstream
  Tauri/Vue GUI tool. Almost all of `bilitools-cli's` business logic
  (WBI signing, aria2c RPC, ffmpeg wrappers, SQLite schema, queue,
  schedulers, login) is a direct port of BiliTools' Rust source.
- **[HKUDS/CLI-Anything](https://github.com/HKUDS/CLI-Anything)** — the
  agentic CLI-replication framework (7-phase SOP: ANALYSIS → DESIGN →
  IMPLEMENT → TEST → DOCUMENT → RELEASE → VERIFY) that produced this
  project. See `ANALYSIS.md` and `DESIGN.md` for the full planning
  artifacts.
- **Bilibili** — for the public API used by both BiliTools and this CLI.
