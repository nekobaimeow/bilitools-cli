# Plan — bilitools `--transcribe` 可选 ASR 功能

> **任务**（用户 2026-06-11 要求）：参考 `https://github.com/nekobaimeow/sensevoice-skill`，给 bilitools 加可选的本地语音转文字功能。**默认不启用**（避免引入 funasr / torch / 900MB 模型 等重依赖），通过 `cargo feature = "transcribe"` 隔离。

---

## 1. 背景 & 设计原则

### 1.1 现状
- `bilitools audio <bv>` 已能下载 m4a（`src/ipc/audio.rs` + `src/cli/audio.rs`）
- 用户问过"能下载音频后续转文字吗"，之前回复"暂未集成"
- sensevoice-skill 项目 **今天 2026-06-11 创建**，在 `https://github.com/nekobaimeow/sensevoice-skill`
- sensevoice-skill 是 **Python CLI 工具**（不在 Hermes 内置 skill 目录里，是独立 git repo + SKILL.md）

### 1.2 关键决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 集成方式 | **subprocess** 调 `python3 sensevoice` | bilitools 是 Rust，funasr/torch 是 Python — 唯一可行边界 |
| 默认开关 | **feature = "transcribe"** | 不污染主 binary（cargo build 默认不拉 Python / 900MB 模型） |
| CLI 形式 | `bilitools audio <bv> --transcribe [--language zh] [--device cpu] [--keep-tags]` | 走 audio 子命令，不开新顶级命令 |
| Python 检测 | `which python3` (clap 已用 sidecar::which 模式) | 复用现有 sidecar 模式 |
| 模型下载 | **不主动下载**，funasr 首次运行自己拉 | bilitools 不背 ~900MB 模型债 |
| 输出格式 | `--json` 含 `{ audio, transcript }`；human 打印路径+字数+段数+耗时 | 跟 audio 子命令风格一致 |

### 1.3 不做的事
- **不**把 sensevoice Python 代码内联到 bilitools
- **不**写 Python FFI
- **不**强制用户装 funasr（subprocess 失败时给清晰错误信息）
- **不**支持云端 ASR（Whisper API 等）— 这是另一条路线

---

## 2. 7 阶段（合并成 3 实际 phase）

### Phase A — Cargo feature 骨架 + ipc/transcribe.rs

#### A.1 `Cargo.toml` 加 feature
```toml
[features]
default = []
transcribe = []   # 启用本地 ASR 子功能 (subprocess → sensevoice-skill CLI)
```

#### A.2 写 `src/ipc/transcribe.rs` (~200 行)

**公开函数**：
```rust
pub struct TranscribeOpts {
    pub m4a_path: PathBuf,
    pub language: String,        // "zh" | "yue" | "en" | "ja" | "ko"
    pub device: String,          // "cpu" | "cuda"
    pub keep_tags: bool,
    pub vad_max_sec: u32,        // 默认 15
    pub sensevoice_cli: PathBuf, // 默认从 SENSEVOICE_CLI env / which "sensevoice" 找
    pub timeout_sec: u64,        // 默认 3600 (1h 音频 ~7min + 余量)
}

pub struct TranscribeResult {
    pub txt_path: PathBuf,
    pub text: String,            // 文件内容
    pub segments: Vec<String>,   // 按行切
    pub char_count: usize,
    pub segment_count: usize,
    pub rtf: f32,                // 推理耗时 / 音频时长
    pub infer_sec: f32,
    pub model: String,           // "iic/SenseVoiceSmall"
    pub device: String,
    pub language: String,
    pub fallback: Option<String>,// python3 没找到 / sensevoice 没装 → 友好提示
}

pub fn transcribe(opts: &TranscribeOpts) -> Result<TranscribeResult, CliError>;
```

**内部逻辑**：
1. `which::which("python3")` 拿 python3 路径
   - 失败 → 返回 `Err` 带安装提示
2. `which::which("sensevoice")` 拿 sensevoice CLI
   - 失败 → 返回 `Err` 带 `pip install ...` / `git clone ...` 提示
3. 构造命令行：`[python3, sensevoice, m4a, -o, txt, -l, lang, -d, dev, ...]`
4. `std::process::Command::new(python3).args(...).output()` 跑
5. parse stdout 拿 `RTF: 0.123` 行（regex）
6. 读 `txt_path` 拿文字内容
7. 返回 `TranscribeResult`

**关键：错误信息设计**
```
[sensevoice] python3 not found in PATH
  → install: sudo apt install python3

[sensevoice] 'sensevoice' CLI not found
  → install: 
      git clone https://github.com/nekobaimeow/sensevoice-skill.git
      cd sensevoice-skill
      pip install funasr numpy soundfile
    or copy `sensevoice` script to ~/.local/bin/

[sensevoice] funasr not installed
  → pip install funasr numpy soundfile

[sensevoice] first run downloads ~900MB model from ModelScope
  → please wait, this only happens once
```

#### A.3 写单元测试 (≥ 4 个)
1. `test_opts_default` — `TranscribeOpts::default()` 字段正确
2. `test_parse_rtf_from_stdout` — regex 抓 "RTF: 0.123"
3. `test_split_segments` — 按行切，空行忽略
4. `test_sensevoice_missing_error` — 友好错误信息
5. `test_sensevoice_invoke_dry_run` — **#[ignore]** 真跑 sensevoice --help（需装好）
6. `test_transcribe_full` — **#[ignore]** 真跑 1 个 m4a（需 model 下载完）

---

### Phase B — CLI 集成 + cargo build/test

#### B.1 改 `src/cli/root.rs` — `Command::Audio` 加可选字段

```rust
#[derive(Args, Debug)]
pub struct AudioArgs {
    pub input: String,
    #[arg(long, default_value = ".")]
    pub output_dir: PathBuf,
    #[arg(long, value_enum, default_value_t = Quality::Qn80)]
    pub quality: Quality,
    // ↓↓↓ 新增 ↓↓↓
    /// Enable local ASR (requires `--features transcribe` + sensevoice-skill)
    #[arg(long)]
    pub transcribe: bool,
    /// Language for ASR (zh/yue/en/ja/ko)
    #[arg(long, default_value = "zh")]
    pub transcribe_language: String,
    /// Inference device (cpu/cuda)
    #[arg(long, default_value = "cpu")]
    pub transcribe_device: String,
    /// Keep emotion tags <|HAPPY|> in transcript
    #[arg(long)]
    pub transcribe_keep_tags: bool,
    /// Override sensevoice CLI path (default: which sensevoice)
    #[arg(long, env = "SENSEVOICE_CLI")]
    pub sensevoice_cli: Option<PathBuf>,
}
```

#### B.2 改 `src/cli/audio.rs` — `--transcribe` 流程

加 1 个 if 分支：
```rust
if cmd.transcribe {
    #[cfg(feature = "transcribe")]
    {
        let opts = TranscribeOpts { ... };
        let result = transcribe::transcribe(&opts)?;
        // human / json 模式
    }
    #[cfg(not(feature = "transcribe"))]
    {
        return Err(CliError::Other(
            "bilitools was built without 'transcribe' feature. \
             Recompile with: cargo build --release --features transcribe"
        ));
    }
}
```

#### B.3 编译验证
- `cargo build --release` ✅ 默认构建无影响
- `cargo build --release --features transcribe` ✅ transcribe 模块编译
- `cargo test --lib` **181/181** 不掉（feature 没开，transcribe.rs 不参与）

---

### Phase C — 文档 + 远端推送

#### C.1 `SKILL.md` 增章
加 `## Audio Transcription` 章节：
- 一句话说明：--transcribe 调本地 sensevoice
- 怎么装 sensevoice-skill（指向那个 repo）
- 使用示例
- 跟 subtitle 一样列在 "When NOT to use"（latency 受限 / 需要云端不要走本地）

#### C.2 `bilitools audio --help` 注释
确保 transcribe 几个 flag 在 help 里可读

#### C.3 git add + commit + push
```
docs+feat: add optional --transcribe flag (sensevoice-skill integration)

* Add Cargo feature 'transcribe' (off by default — no Python/funasr dep)
* New ipc::transcribe module — subprocess invoke sensevoice CLI
* bilitools audio <bv> --transcribe [--language zh] [--device cpu]
* Update SKILL.md with Audio Transcription section
* Error paths: python3 missing / sensevoice missing / funasr missing
  all give actionable install hints
```

---

## 3. 风险 & 缓解

| 风险 | 缓解 |
|------|------|
| subprocess hang | `--timeout` + 友好错误（"audio > 1h, raise --transcribe-timeout"） |
| 模型下载慢 (~900MB) | 不在 bilitools 范围，由 sensevoice 自己管 |
| 默认 binary 用户误用 --transcribe | 编译时 feature gate, 没开就用红色错误信息 |
| 输出文件中文名 | 跟 audio 一样 sanitize，bilitools 不背 Unicode 债 |
| funasr 装失败 | Python 端处理，bilitools 只透传 stderr |

---

## 4. 验收清单

- [ ] `cargo build --release` 通过（无 feature）
- [ ] `cargo build --release --features transcribe` 通过
- [ ] `cargo test --lib` ≥ 181/181（默认 feature）
- [ ] `cargo test --lib --features transcribe` ≥ 185/185
- [ ] 单元测试 `test_parse_rtf_from_stdout` ✅
- [ ] 单元测试 `test_sensevoice_missing_error` ✅
- [ ] SKILL.md 新增 `Audio Transcription` 章节 ✅
- [ ] git commit + push `eeda0ed..HEAD` ✅

---

## 5. 不在范围

- ❌ Whisper / 云端 ASR 集成（另一路线）
- ❌ 实时流式 ASR（latency 不行）
- ❌ 多 ASR 引擎切换 UI（CLI --engine 留 TODO）
- ❌ 把 sensevoice Python 内嵌 bilitools
- ❌ 自动下载模型（funasr 自己管）

---

## 6. 串行执行顺序

```
A.1 → A.2 → A.3 → cargo build --features transcribe
   → B.1 → B.2 → B.3 → cargo build (default)
   → C.1 → C.2 → git commit + push
```

每个文件改完单独 cargo build/test 一次。
