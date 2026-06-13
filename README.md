
<p align="center">
  <img src="https://img.shields.io/crates/v/bilicli?style=flat-square&color=success" alt="crates.io">
  <img src="https://img.shields.io/github/license/nekobaimeow/bilicli?style=flat-square" alt="license">
  <img src="https://img.shields.io/badge/language-Rust-orange?style=flat-square" alt="Rust">
</p>

# bilicli — Multi-Platform Video Toolkit CLI

> *Born from BiliTools. Expanding to the world.*

`bilicli` is a **100% Rust** CLI toolkit for video platforms — download, analyze, transcribe, OCR,
and batch-harvest video metadata. It started as a pure CLI port of
[BiliTools](https://github.com/btjawa/BiliTools) but has grown into its own project with an
ambitious multi-platform roadmap.

🔹 **简体中文** · 🔹 **[English](#english)** · 🔹 **[日本語](#japanese)**

---

## 简体中文

### 🚀 Quickstart

```bash
# 安装 (via crates.io)
cargo install bilicli

# 或从源码编译
git clone https://github.com/nekobaimeow/bilicli
cd bilicli
cargo build --release
```

### 📋 系统依赖

`bilicli` 需要三个外部工具，请用系统包管理器安装：

| 工具 | 用途 | 安装 |
|------|------|------|
| **aria2c** | 多线程下载 | `apt install aria2` / `brew install aria2` |
| **ffmpeg** | 音视频合并 | `apt install ffmpeg` / `brew install ffmpeg` |
| **python3** | ASR 语音转文字 | `apt install python3` / 通常已安装 |

```bash
# 一键体检
bilicli doctor

# 自动安装 ASR 模型 + 依赖
bilicli setup
```

### 🎯 核心功能

```bash
# 扫码登录 B 站
bilicli auth qrcode

# 下载视频
bilicli download submit "https://www.bilibili.com/video/BV1xx411c7mD" --quality 80

# 全量分析：ASR 语音识别 + 弹幕 + 字幕 + 评论 + OCR 逐帧字幕
bilicli analyze BV1xx411c7mD

# 离线 OCR（提取视频硬字幕）
bilicli ocr input.mp4 --video

# 弹幕导出
bilicli danmaku BV1xx411c7mD --format xml -o ./danmaku.xml

# 批量采集
bilicli harvest --search "原神 演示" --limit 50
```

### 🗺️ Roadmap

| 状态 | 功能 |
|------|------|
| ✅ 已完成 | Bilibili 全功能（下载/弹幕/评论/字幕/ASR/OCR/REPL） |
| 🔨 开发中 | YouTube 支持 |
| 📋 计划中 | TikTok（抖音）、Niconico |
| 📋 计划中 | Google Drive / 百度网盘 / 夸克网盘 云端存储集成 |
| 💭 探索中 | 跨平台搜索、P2P 分发 |

### 📁 数据目录

```
Linux:   $XDG_DATA_HOME/com.nekobaimeow.bilicli/
macOS:   ~/Library/Application Support/com.nekobaimeow.bilicli/
Windows: %AppData%\com.nekobaimeow.bilicli\
```

### 📝 致谢

本项目脱胎于 [btjawa/BiliTools](https://github.com/btjawa/BiliTools) — 致敬原作者的开创性工作。
bilicli 复用了其 Rust 业务逻辑（登录、aria2c、ffmpeg、队列、数据库）并拓展了 OCR、ASR、
及多平台兼容能力。

感谢所有贡献者和开源生态。

---

## English

### 🚀 Quickstart

```bash
# Install via crates.io
cargo install bilicli

# Or build from source
git clone https://github.com/nekobaimeow/bilicli
cd bilicli
cargo build --release
```

### 📋 Dependencies

| Tool | Purpose | Install |
|------|---------|---------|
| **aria2c** | Multi-threaded download | `apt install aria2` / `brew install aria2` |
| **ffmpeg** | Audio/video muxing | `apt install ffmpeg` / `brew install ffmpeg` |
| **python3** | ASR speech-to-text | `apt install python3` (usually pre-installed) |

```bash
# Health check
bilicli doctor

# Auto-install ASR models & Python deps
bilicli setup
```

### 🎯 Features

```bash
bilicli auth qrcode          # QR login to Bilibili
bilicli download submit <url>  # Download video
bilicli analyze <BV>         # Full analysis: ASR + danmaku + subs + comments + OCR
bilicli ocr input.mp4 --video # Offline OCR (extract hardcoded subtitles)
bilicli danmaku <BV>         # Export danmaku (xml/json/ass)
bilicli harvest --search <q> # Batch collect search results
bilicli repl                 # Interactive shell
```

### 🗺️ Roadmap

| Status | Feature |
|--------|---------|
| ✅ Done | Bilibili full stack (download, danmaku, comments, subs, ASR, OCR, REPL) |
| 🔨 WIP | YouTube support |
| 📋 Planned | TikTok, Niconico |
| 📋 Planned | Cloud storage: Google Drive, Baidu Netdisk, Quark Netdisk |
| 💭 Exploring | Cross-platform search, P2P distribution |

### 🙏 Credits

This project is derived from [btjawa/BiliTools](https://github.com/btjawa/BiliTools).
We reuse its Rust business logic and extend it with OCR, ASR, and multi-platform ambitions.
Deep gratitude to the original author and all open-source contributors.

---

## 日本語

### 🚀 クイックスタート

```bash
# crates.io からインストール
cargo install bilicli

# またはソースからビルド
git clone https://github.com/nekobaimeow/bilicli
cd bilicli
cargo build --release
```

### 📋 依存関係

| ツール | 用途 | インストール |
|--------|------|-------------|
| **aria2c** | マルチスレッドダウンロード | `apt install aria2` / `brew install aria2` |
| **ffmpeg** | 音声/動画の結合 | `apt install ffmpeg` / `brew install ffmpeg` |
| **python3** | 音声認識（ASR） | `apt install python3`（通常インストール済み） |

```bash
bilicli doctor   # 環境チェック
bilicli setup    # ASR モデルと依存関係の自動インストール
```

### 🎯 主な機能

```bash
bilicli auth qrcode           # QR コードで Bilibili にログイン
bilicli download submit <url> # 動画をダウンロード
bilicli analyze <BV>          # フル分析: ASR + 弾幕 + 字幕 + コメント + OCR
bilicli ocr input.mp4 --video # オフライン OCR（ハードサブタイトルの抽出）
bilicli danmaku <BV>          # 弾幕をエクスポート (xml/json/ass)
bilicli harvest --search <q>  # 検索結果の一括収集
bilicli repl                  # インタラクティブシェル
```

### 🗺️ ロードマップ

| 状態 | 機能 |
|------|------|
| ✅ 完了 | Bilibili フル対応（DL・弾幕・コメント・字幕・ASR・OCR・REPL） |
| 🔨 開発中 | YouTube 対応 |
| 📋 計画中 | TikTok（抖音）、Niconico |
| 📋 計画中 | クラウド連携: Google Drive、百度網盤、夸克網盤 |
| 💭 検討中 | クロスプラットフォーム検索、P2P 配信 |

### 🙏 謝辞

本プロジェクトは [btjawa/BiliTools](https://github.com/btjawa/BiliTools) から派生しました。
原作者の先駆的業績に深く感謝します。bilicli は BiliTools の Rust ビジネスロジックを活用し、
OCR・ASR・マルチプラットフォーム対応へと拡張しています。

---

## 📄 License

GPL-3.0-or-later — same as upstream [BiliTools](https://github.com/btjawa/BiliTools).

<p align="center">
  <sub>Built with 🦀 Rust · Powered by open source</sub>
</p>
