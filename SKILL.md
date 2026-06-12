---
name: bilitools
description: "Use this skill whenever the user needs to download, search, or inspect content from Bilibili (B 站). Triggers: user mentions B 站, Bilibili, BV/av/SS/EP, 弹幕, 字幕, 评论, 封面, UP 主, 番剧, 课堂, "看视频" (analyze a video), "分析这个 B 站视频" (analyze this B 站 video), "帮我扒一下" (scrape/extract), or wants to fetch danmaku / comments / subtitles / audio transcript / video metadata / search results from bilibili.com. This skill wraps the `bilitools` CLI which is a pure-Rust port of the BiliTools GUI; it covers video download (DASH segments via aria2c + ffmpeg merge), audio-only download (m4a) + optional local ASR (sensevoice), danmaku (XML or ASS via DanmakuFactory), comments (hot + time + sub-replies), subtitles (JSON, requires login + wbi sign), search (video / bangumi / user / cheese course aware), and a `harvest` batch that does all four for top-N search results. Use this whenever B 站 / Bilibili content extraction or text-based video analysis is the task. Do NOT use for non-Bilibili video sites (YouTube, Vimeo, etc.) or for posting content back to B 站."
license: GPL-3.0-or-later
---

# bilitools — Bilibili CLI Toolkit

## Overview

`bilitools` is a pure-Rust CLI port of the [BiliTools](https://github.com/btjawa/BiliTools) GUI app.
It exposes **17 subcommands** for searching, downloading, and inspecting B 站 content from the
terminal or from another AI agent. The original Rust backend logic (WBI signing, buvid
fingerprinting, aria2 RPC, ffmpeg merge, SQLite task queue) is reused unchanged; only the
Tauri GUI layer was stripped.

**When to use:**
- User wants to **analyze / "watch" a B 站 video by extracting its text** — the standard
  "看视频" workflow: search → metadata → danmaku + comments + subtitles + audio transcript
  (see [Analyzing a B 站 Video](#analyzing-a-b-站-video-the-standard-看视频-workflow))
- User wants to download a B 站 video (single, batch, or scheduled)
- User wants just the audio track of a B 站 video (m4a, no video)
- User wants danmaku, comments, or subtitles extracted to JSON / XML / ASS
- User wants to search B 站 from the terminal and dump results to JSON
- User wants to do all of the above for the top-N results of a search (`harvest`)

**When NOT to use:**
- Other video sites (YouTube → `yt-dlp`, Vimeo, etc.)
- Posting or interacting with B 站 (uploading, liking, commenting)
- **Actually playing back video** — this skill is for text-based extraction, not
  media playback. If the user wants to *watch* the video, this skill harvests the text
  (弹幕 / 评论 / 字幕 / 音频转录) so the agent can summarize; the user still watches
  the video themselves in a browser.

## Quick Reference

| Task | Command |
|------|---------|
| Search "黄金" top 5 | `bilitools search "黄金" --limit 5` |
| Fetch comments for a video | `bilitools review BV1CZEY67E8o` |
| Fetch danmaku (XML + ASS) | `bilitools danmaku BV1CZEY67E8o` |
| Fetch subtitles | `bilitools subtitle BV1XBRuBSEd7 --download -o /tmp/subs` |
| Download audio only (m4a) | `bilitools audio BV1XBRuBSEd7 -o ~/Music` |
| Download full video (1080P) | `bilitools download submit BV1XBRuBSEd7` + `bilitools download run <task_id>` |
| Batch: top 5 黄金 results → danmaku + comments + subs | `bilitools harvest "黄金" --limit 5 -o ./out` |
| Check login + sidecars | `bilitools doctor` |
| Log in (QR code) | `bilitools auth qrcode -o qr.png` + `bilitools auth qrcode-poll <key>` |
| **OCR** a screenshot or video frame (PP-OCRv5) | `bilitools ocr screenshot.png` / `bilitools ocr video.mp4 --video` (opt-in `--features ocr`) |
| **Analyze a video** (搜 → 元数据 → 弹幕/评论/字幕/音频转录/OCR) | see [standard workflow below](#analyzing-a-b-站-video-the-standard-看视频-workflow) |

All subcommands accept `--json` for machine-readable output. **Always prefer `--json` when
this skill is being driven by another agent** (table output is for humans, JSON is stable).

## Installation

The `bilitools` binary must be on `$PATH` (or at `~/.cargo/bin/bilitools`).

```bash
# Build from source
git clone https://github.com/nekobaimeow/bilitools-cli
cd bilitools-cli
cargo build --release
sudo cp target/release/bilitools /usr/local/bin/

# Verify
bilitools --version
bilitools doctor   # checks aria2c + ffmpeg + DanmakuFactory + B 站 API
```

**Required sidecars** (all `which`-detectable, override via `BILITOOLS_SIDECAR_*` env):
- `aria2c` — DASH segment downloads (parallel + resumable)
- `ffmpeg` — final mp4 muxing + m4a extraction

**Optional sidecar:**
- `DanmakuFactory` — XML → ASS conversion. Without it, danmaku saves as raw XML only.

## Authentication

Most endpoints work **better** when logged in. Login state lives in the SQLite database at
`$XDG_DATA_HOME/com.btjawa.bilitools/Storage/storage.db` (or `BILITOOLS_DATA_DIR/Storage/`).

```bash
# 1. Generate a QR code PNG
bilitools --json auth qrcode -o /tmp/qr.png
# → {"data": {"qrcode_key": "...", "url": "..."}, ...}

# 2. User scans the QR with the B 站 mobile app

# 3. Poll until login succeeds
bilitools auth qrcode-poll <qrcode_key>

# 4. Verify
bilitools --json auth status
# → {"data": {"cookies": ["SESSDATA", "DedeUserID", "bili_jct", ...], "logged_in": true}}
```

Anonymous mode (no SESSDATA) still works for most read endpoints, but B 站 applies rate
limits (e.g. comments are capped at 3-5 per page, subtitles are usually empty).

## Subcommand Catalog

### `search` — Search B 站

```bash
bilitools search "原神 演示" --limit 5
bilitools --json search "原神 演示" --type bangumi --page 1 --page-size 20
```

- `--type` accepts: `video` (default), `bangumi`, `user`, `article`, `audio`, `live`, `topic`
- Returns: `keyword`, `page`, `total`, `results[]` (each with `bvid` / `ssid` / `cheese:ss{N}` discriminator, `title`, `author`, `play`, `pubdate`)
- Cheese 课堂 courses are returned as `kind="cheese"` with `bvid=null` and `ssid="N"`. The
  table column shows `cheese:ss{N}` for human eyes.

### `danmaku` — Fetch danmaku

```bash
bilitools danmaku BV1R1e4zKEh1 --format both -o /tmp/dm
# Writes /tmp/dm/{cid}.xml and /tmp/dm/{cid}.ass (if DanmakuFactory installed)
```

- `--source live` (default) / `history` / `both`. History requires protobuf parser (not yet).
- `--format xml` / `ass` / `both` (default).
- Counts: `live_count`, optional `history_count` (currently 0).
- Output JSON includes `xml_path`, `ass_path`, `danmakufactory_used`, `degraded[]`.

### `review` — Fetch comments

```bash
bilitools review BV1CZEY67E8o --sort hot --ps 10
bilitools --json review BV1CZEY67E8o --sort time --page 2 --ps 20
bilitools review BV1CZEY67E8o --sub 305363471760  # fetch sub-replies for one rpid
```

- `--sort hot` (default, `sort=2`) or `time` (`sort=0`).
- `--ps K` capped at 30 server-side; anonymous mode auto-capped to 3-5.
- Recursive: top-level `replies[]` each have their own `sub_replies[]` and `sub_replies_count`.
- IDs in JSON: `rpid` (root), `mid` (user), `uname`, `avatar`, `message`, `like`, `ctime`.

### `subtitle` — Fetch subtitles

```bash
bilitools --json subtitle BV1XBRuBSEd7                 # list metadata only
bilitools subtitle BV1XBRuBSEd7 --download -o /tmp/s  # download JSON bodies
```

**IMPORTANT** — this subcommand uses WBI signing internally. B 站 has two subtitle fields:
- `data.subtitle.list[]` — public, **always empty for non-browser clients**
- `data.subtitle.subtitles[]` — WBI-signed, **real data**

`bilitools` does the WBI signing automatically (via `shared::wbi_sign()`) and reads
`subtitles[]`. Without SESSDATA, the API will return 0 entries even for videos that have
subtitles. If you see `[info] no subtitles available`, log in first.

JSON files land as `{subtitle_id}.{lan}.json` in the output dir. B 站's body is
`{"body": [{"from": 0.4, "to": 2.5, "content": "..."}, ...]}`.

### `audio` — Download audio track only (m4a)

```bash
bilitools audio BV1XBRuBSEd7 -o ~/Music/bili
bilitools --json audio BV1CZEY67E8o -q 16  # 360P tier; audio bitrate chosen by B 站

# Optional: also produce a local transcript (requires --features transcribe
# at build time AND the external `sensevoice` CLI on PATH — see
# "Audio Transcription" below).
bilitools audio BV1XBRuBSEd7 --transcribe -o ~/podcast
bilitools audio <bv> --transcribe --transcribe-language en --transcribe-device cuda
```

- DASH audio segment → reqwest download (no aria2c overhead for single file) → ffmpeg
  `-vn -c:a copy` → `.m4a`.
|- Output: `{sanitize(title)}-{cid}.m4a`. Non-ASCII characters (Chinese etc.) become
  `_`; ASCII letters, digits, and a few separators (`- _ .`) are preserved. The CID is
  always appended for uniqueness. (E.g. `2026.6.11黄金…IPO…` →
  `2026.6.11_______________________4024______________________________IPO_________-39024790178.m4a`.)
- Use case: offline listening, speech-to-text post-processing (Whisper, MiniMax, **sensevoice**, etc.).

### `download` — Full video download (DASH)

```bash
# 1. Submit a task
bilitools --json download submit BV1XBRuBSEd7 --output-dir ~/Videos/bili
# → {"data": {"task_id": "uuid", ...}}

# 2. Run it (DASH video + audio segments + ffmpeg merge → mp4)
bilitools --json download run <task_id>
# → {"data": {"output": "/path/to/merged.mp4", "segments": [...], ...}}

# Or batch
bilitools download batch urls.txt   # one URL per line
bilitools download list            # list all tasks
bilitools download show <id>       # task details
bilitools download cancel / pause / resume / retry
```

`--quality 80` (1080P), `64` (720P), `32` (480P), `16` (360P). Default 80. Audio is picked
by B 站 independently of `--quality`.

### `harvest` — Batch all-in-one for top-N search results

```bash
bilitools harvest "黄金" --limit 5 -o ./out
bilitools harvest "原神" --limit 3 --no-danmaku --no-review --no-subtitle
```

- Runs `search` → for each top-N: `danmaku` + `review` + `subtitle` (or subset via flags).
- One subdirectory per video: `{output}/{slug-title}/` containing:
  - `{cid}.xml` (danmaku, if enabled)
  - `{id}.{lan}.json` (subtitle, if any)
  - `review.json` (full comment tree, if enabled)
  - `meta.json` (BV, aid, cid, title, harvested_at)
- `--limit 5` is *requested* but B 站's `page_size` is sometimes capped at 20 server-side.

### Other subcommands

| Cmd | Purpose |
|-----|---------|
| `info` | Print version + paths + build info |
| `init` | Refresh buvid3 / buvid4 / bili_ticket / _uuid (now an alias; context auto-loads) |
| `auth qrcode / qrcode-poll / status / refresh / exit` | Login lifecycle |
| `parse url/bv/av/bangumi/episode/fav/watchlater/user` | Inspect a B 站 resource without downloading |
| `schedule list/add/remove/run` | Cron-based downloads |
| `config show/get/set/reset/path` | Inspect & modify TOML config |
| `cache list/size/clean/open` | Cache directory management |
| `db export/import/tasks` | SQLite task DB management |
| `doctor` | Health check (sidecars + B 站 nav API) |
| `repl` | Interactive REPL (rarely needed; CLI is friendlier) |

## Analyzing a B 站 Video (the standard "看视频" workflow)

When the user gives you a B 站 URL (or BV / av / SS / EP id, or a search theme) and says
something like "看这个视频", "分析一下", "帮我扒一下里面的内容", "总结一下讲的啥",
**do not try to play the video**. You harvest every text source B 站 exposes, then
synthesize.

### Phase 0 — Resolve the input

The input may be any of:

| Input form | Action |
|------------|--------|
| Full URL `https://www.bilibili.com/video/BV...` | Strip scheme/host; use the id |
| Bare `BV1CZEY67E8o` / `av170001` / `ss12345` / `ep67890` | Use as-is |
| Search theme "黄金价格 2026" | Run `search` first, present top results, ask user to pick or auto-pick #1 |
| Fuzzy "昨天那个视频" | If you have session history pointing at a BV, use it; else ask |

`bilitools parse` accepts all the above forms and returns the canonical `{bvid, aid, cid,
title, duration, ...}`. Use it as a normalization step before anything else.

```bash
bilitools --json parse url "https://www.bilibili.com/video/BV1CZEY67E8o"
# → {"data": {"bvid": "BV1CZEY67E8o", "aid": ..., "cid": ..., "title": "...", ...}}
```

### Phase 1 — Always-run text harvests (no audio, fast)

These four subcommands cover **what was said** in the video. Run them in parallel (they
are independent):

```bash
# 1a. Metadata (title, UP 主, desc, tags, stats, cid for the others)
bilitools --json parse url BV... | jq '.data | {bvid,title,author,desc,pubdate,duration,stat}'

# 1b. 弹幕 — real-time viewer reactions; for analysis, prefer `--source live` XML
bilitools --json danmaku BV... --format xml --source live -o /tmp/dm

# 1c. 评论 (top + recent) — usually more analytical than danmaku
bilitools --json review BV... --sort hot --ps 30
bilitools --json review BV... --sort time --ps 30  # secondary pass for recency

# 1d. 字幕 — the most accurate transcript (B 站's own AI or human subs)
bilitools --json subtitle BV... --download -o /tmp/subs
```

If `danmaku` / `review` / `subtitle` return empty / 0 entries, run `bilitools auth
status` — the most common cause is **not being logged in** (anonymous mode caps
subtitles and comments). If unauthenticated, stop and tell the user to scan a QR.

### Phase 2 — Optional: audio → transcript (when subs are missing or wrong)

If `subtitle` returned 0 entries, OR the user wants **verbatim** speech-to-text
(dialogue, slang, music lyrics), fall back to downloading audio and running ASR:

```bash
# 2a. Download m4a only (no video, fast)
bilitools --json audio BV... -o /tmp/audio

# 2b. (optional) transcribe via sensevoice — requires --features transcribe at build
#     AND the `sensevoice` CLI on PATH; see "Audio Transcription" below
bilitools --json audio BV... --transcribe -o /tmp/audio
```

If the binary was NOT built with `--features transcribe`, OR `sensevoice` is missing,
say so clearly and offer the user the `.m4a` path so they can run their own ASR.
**Do not silently skip** — tell the user what you couldn't do and why.

### Phase 2c — OCR (opt-in): hard-coded text in frames

B 站's own subtitle pipeline misses a lot of visual text — title cards, watermarks,
on-screen labels, in-frame Chinese/English, foreign-language text without audio, etc.
`bilitools ocr` runs **PP-OCRv5 mobile (FP16, MNN backend)** fully offline to catch
the rest. Requires the binary to be built with `--features ocr` AND MNN model files
in `models/ocr-fast/`. See the **OCR subcommand** section below.

Use OCR when:
- subtitle came back empty AND audio has no useful speech
- the user asks about on-screen text, watermarks, title cards, foreign captions
- you want chapter titles from the video itself (e.g. 旅行 vlog 章节标题)

Skip OCR when:
- subtitle already covers the dialogue (don't duplicate work)
- the user only cares about spoken content (audio transcription is enough)

### Phase 3 — Synthesize (this is the actual "watching")

Once you have the four text sources, read them and produce the analysis. Suggested
structure for the final reply:

1. **One-line summary** — what the video is, who made it, when
2. **Outline / chapters** — derive from subtitle timestamps or desc structure
3. **Key claims / facts** — from subtitle body, with original phrasing quoted
4. **Viewer pulse** — top themes from hot comments + high-frequency danmaku
   (count occurrences, not just the first hit)
5. **Disagreements / controversy** — flagged from comments with `like > N` or
   sub-replies > 0
6. **Open questions / TODO** — anything you couldn't extract (e.g. visual-only
   content, missing subs, foreign language you can't transcribe)

### Concurrency & error handling

- Phases 1a–1d are **independent** — fire them in parallel (delegate_task batch
  or background processes). Total wall time ≈ slowest single call, not sum.
- If `danmaku` fails with a CID mismatch, re-run `parse` — B 站 sometimes returns
  the wrong CID for bangumi/UGC; the XML endpoint will 404 on bad CID.
- If `subtitle` returns a list but `--download` errors on one language, the others
  are usually still fine — check `data.degraded[]` and continue.
- Cap `danmaku` reads at ~50k entries for a 2h video; above that, sample by time
  bucket (e.g. first/last/middle 5 min) rather than reading the whole XML.

### One-liner: "do everything" via harvest

For top-N search results, `harvest` does Phases 1a–1d in one call:

```bash
bilitools harvest "黄金价格" --limit 5 -o ./gold-batch
# Produces 5 subdirs: <title>/{danmaku.xml, review.json, subtitle-*.json, meta.json}
```

For a single video, you can fake the same shape with:

```bash
mkdir -p /tmp/one/{dm,subs}
bilitools danmaku BV... -o /tmp/one/dm
bilitools subtitle BV... --download -o /tmp/one/subs
bilitools review BV... --json > /tmp/one/review.json
bilitools --json parse url BV... > /tmp/one/meta.json
```

## Common Workflows

### 1. Save a video + comments + subs for offline reading

```bash
# Login once
bilitools auth qrcode -o /tmp/qr.png && bilitools auth qrcode-poll $KEY

# Search & download
bilitools --json search "AI 教程" --limit 3
TID=$(bilitools --json download submit BV... | jq -r .data.task_id)
bilitools --json download run $TID

# Annotate the downloaded video
bilitools subtitle BV... --download -o /tmp/subs
bilitools review BV... --ps 20 --json > /tmp/comments.json
bilitools danmaku BV... --format xml -o /tmp/dm
```

### 2. Quick "watch later" — just the audio

```bash
bilitools audio BV... -o ~/Music/podcast
# Then in your podcast player, point at ~/Music/podcast
```

### 3. Speech-to-text pipeline (local, offline, no API key)

bilitools integrates with [sensevoice-skill](https://github.com/nekobaimeow/sensevoice-skill),
a one-shot Python CLI wrapping Alibaba's SenseVoiceSmall. RTF ~0.12 on CPU, 900 MB
one-time model download, supports zh / yue / en / ja / ko.

```bash
# Step 0: install (one time) — Python deps + model
pip install funasr numpy soundfile
git clone https://github.com/nekobaimeow/sensevoice-skill
# (puts a `sensevoice` script in the repo; either put it on PATH or pass
#  --sensevoice-cli /full/path/to/sensevoice to bilitools)

# Step 1: rebuild bilitools with the optional transcribe feature
cargo install --path . --features transcribe

# Step 2: download audio + transcribe in one shot
bilitools audio BV1XBRuBSEd7 --transcribe -o ~/podcast
#   → ~/podcast/<title>-<cid>.m4a         (audio)
#   → ~/podcast/<title>-<cid>_文字稿.txt   (transcript, 4633 chars / 10min)
```

For **other** ASR backends (whisper.cpp, MiniMax API, etc.), bilitools does not bundle them
— feed the `.m4a` straight into your own tool.

### 4. Search → top 5 → save everything

```bash
bilitools harvest "TED 演讲" --limit 5 -o ./ted-batch
# Produces 5 subdirs each with danmaku.xml + review.json + subtitle(s) + meta.json
```

## Output Conventions

- **Human mode** (default): pretty tables with column headers, status messages to stdout.
  Designed for terminals. INFO-level log lines (timestamps) also go to stdout — they will
  pollute JSON parsers. Use `--json` to suppress.

- **JSON mode** (`--json`): single line of valid JSON with shape:
  ```json
  {"ok": true, "data": {...}}
  ```
  Errors:
  ```json
  {"ok": false, "error": {"code": "API", "message": "..."}}
  ```
  **When driving bilitools from another agent, ALWAYS use `--json`** and parse with `jq` /
  Python `json` to avoid log-line pollution.

- **Exit code**: `0` on success, `1` on any error. Some subcommands return `0` even with
  partial failures (e.g. `harvest` when some videos failed) — check `data.degraded[]`.

## Audio Transcription (Optional)

Optional local ASR via [sensevoice-skill](https://github.com/nekobaimeow/sensevoice-skill)
— **Alibaba SenseVoiceSmall** model, CPU inference, no API key, no cloud upload.

This is an **opt-in feature** to keep the default binary lean. Two conditions must
be true simultaneously for it to work:

1. **bilitools was built with `--features transcribe`**
2. **The `sensevoice` script is on PATH** (or pass `--sensevoice-cli /path`)

Without either, the user gets a clear error message at runtime — bilitools will
**never** silently no-op.

### Build

```bash
cargo install --path /path/to/bilitools-cli --features transcribe
# or, from the repo:
cargo build --release --features transcribe
```

The `transcribe` feature only pulls in a tiny `which` crate (~30 KB) for locating
the `python3` and `sensevoice` executables. It does NOT bundle funasr / torch /
the 900 MB model — those are installed in the **next** step.

### Install sensevoice (one time)

```bash
pip install funasr numpy soundfile
git clone https://github.com/nekobaimeow/sensevoice-skill
cd sensevoice-skill && chmod +x sensevoice
# Option A: put it on PATH
sudo ln -s "$(pwd)/sensevoice" /usr/local/bin/sensevoice
# Option B: pass full path each invocation (no PATH change)
bilitools audio <bv> --transcribe --sensevoice-cli /full/path/to/sensevoice
```

First `sensevoice` run downloads `iic/SenseVoiceSmall` (~900 MB) from ModelScope.
Subsequent runs use the cache at `~/.cache/modelscope/`.

### Usage

```bash
# Download audio + transcribe in one shot
bilitools audio BV1XBRuBSEd7 --transcribe -o ~/podcast

# Language / device / tag options
bilitools audio <bv> --transcribe --transcribe-language en
bilitools audio <bv> --transcribe --transcribe-device cuda        # needs CUDA torch
bilitools audio <bv> --transcribe --transcribe-keep-tags          # keep <|HAPPY|>

# JSON output (transcript printed as a second JSON line — jq -s to combine)
bilitools --json audio <bv> --transcribe | jq -s '.[0].audio + .[1].transcript'
```

### Flags (audio subcommand)

| Flag | Default | Description |
|------|---------|-------------|
| `--transcribe` | off | Run sensevoice after the m4a is downloaded |
| `--transcribe-language <auto\|zh\|yue\|en\|ja\|ko>` | `auto` | Language hint for ASR. `auto` = don't pass `-l` to sensevoice; the multilingual model detects per segment (best for B 站's typical zh+en mix) |
| `--transcribe-device <cpu\|cuda>` | `cpu` | Inference device |
| `--transcribe-keep-tags` | off | Keep `<|HAPPY|>` emotion tags in transcript |
| `--sensevoice-cli <path>` | `which sensevoice` | Override the sensevoice script path |

### Output

A successful run produces two files in `-o`:

- `<title>-<cid>.m4a` — the audio (unchanged from non-transcribe flow)
- `<title>-<cid>_文字稿.txt` — Chinese-text-named transcript, one sentence per
  line. With `--transcribe-keep-tags`, lines are prefixed with `<|zh|><|HAPPY|>` etc.

### Performance (CPU, 14-core x86)

| Audio | sensevoice wall time | RTF |
|-------|---------------------|-----|
| 10 min | ~100 s | 0.158 |
| 22 min | ~160 s | 0.12 |
| 60 min | ~7 min | 0.12 |

### Common errors

| Error message | Fix |
|---------------|-----|
| `python3 not found in PATH` | `sudo apt install python3` |
| `the sensevoice CLI is not on PATH` | Install per instructions above |
| `ModuleNotFoundError: No module named 'funasr'` | `pip install funasr numpy soundfile` |
| `bilitools was built without the 'transcribe' feature` | `cargo install --path . --features transcribe` |
| `sensevoice timed out after Ns` | First run downloads ~900 MB; raise timeout or be patient |

### When to use sensevoice vs. alternatives

- **sensevoice**: best for **Chinese** (zh / yue); very fast on CPU; fully offline.
- **whisper.cpp / OpenAI Whisper**: better for **English-only** and high accuracy.
- **Cloud API** (Deepgram, AssemblyAI, MiniMax ASR): best for low-latency streaming,
  multilingual noise robustness, speaker diarization.

bilitools only bundles the bilitools↔sensevoice integration. For Whisper / cloud,
call those tools directly on the `.m4a` output.

## Known Pitfalls

1. **WBI signing is required for `subtitle` and `playurl`.** These endpoints return 0
   entries or fail with -352 "风控校验失败" without it. BiliTools handles this
   automatically; don't try to call the raw HTTP endpoints without WBI signing.

2. **Search API returns `subtitle: ""` in the result row** — this is *not* the subtitle
   field the `subtitle` subcommand reads. It's a search-result string. Don't be fooled.

3. **`/x/player/wbi/v2` has two subtitle fields.** `list[]` is the public decoy
   (always empty); `subtitles[]` is the real one. The CLI handles this; if you're
   debugging the API directly, read the right field.

4. **Anonymous = rate-limited.** Without SESSDATA: comments 3-5/page, subtitles usually
   0, audio/video may need login for 1080P+. Log in via `bilitools auth qrcode` for full
   access.

5. **4K HDR is mostly marketing.** B 站 returns 4K in `accept_quality[]` for many videos
   but the actual stream maxes out at 1080P. `qn=80` is the real ceiling for most
   non-UPower-paid content. Setting `--quality 120` may fall back to 112 or 80.

6. **`--limit` on `harvest` is best-effort.** B 站's `page_size` param is sometimes
   honored, sometimes capped at 20. The `harvest` walks whatever the API returns.

7. **Output file names** sanitize non-ASCII (Chinese etc.) → `_` and truncate at 80
   chars. ASCII letters / digits / a few separators (`- _ .`) are kept. The CID is
   always appended for uniqueness: `{slug}-{cid}.m4a`. Long Chinese titles can produce
   a wall of underscores — use `--json` to get the original title in the response.

7a. **`danmaku` fetches `comment.bilibili.com/<cid>.xml`, which since ~2024 returns
   raw deflate (no zlib/gzip header, no Content-Encoding header) instead of plain
   XML.** reqwest's global `Client` builder enables `.gzip(true).deflate(true)` for
   transparent decoding; on this URL it mis-fires on the Content-Type=text/xml
   response and surfaces as `error decoding response body`. `Accept-Encoding:
   identity` does NOT prevent the transparent decode (it only affects what the
   server sends). The CLI now builds a dedicated, gzip/deflate-disabled reqwest
   client for the danmaku URL and manually raw-deflate-decodes the body. **Same fix
   pattern was already needed for `view` (search.rs) and `subtitle` headers
   (search.rs) but the danmaku one was the only place that actually surfaced as a
   user-visible error** — both other endpoints happen to be JSON, which reqwest
   recovers from via the "looks like JSON" sniff. The XML body has no JSON sniff
   fallback, so it died loudly.

8. **`--transcribe` requires two opt-ins.** The bilitools binary must be built with
   `--features transcribe` AND the `sensevoice` Python CLI must be on PATH (or
   passed via `--sensevoice-cli`). See "Audio Transcription" above. bilitools
   will refuse with a clear error rather than silently no-op.

9. **ASR may mangle proper nouns.** SenseVoiceSmall is a 240 M-param model; it
   routinely mishears foreign names ("Hegseth" → "黑克萨斯 / 黑格莱斯 / 黑克赛斯",
   3 different spellings across the same video). English technical terms often
   survive intact (SSNX, 095, 3D), but Chinese proper nouns and romanized
   foreign names are lossy. Treat transcripts as ~95% accurate, not 100%. If
   verbatim quoting matters, cross-check against the B 站 AI subtitles (which
   bilitools already harvests in `harvest`).

## OCR (Optional — extract hard-coded text from images or video)

Offline OCR via **PaddleOCR PP-OCRv5 mobile (FP16) + MNN** — the same model
that ships with the rpic Windows image viewer (which we adapted and
validated on B 站 video content). **Pure Rust, no Python process.**

Use cases:

- Video has no B 站 AI subtitle and no useful speech → OCR the on-screen
  chapter titles / watermarks / title cards instead
- Foreign-language video with on-screen translated text (e.g. 旅行 vlog with
  English/Chinese overlay)
- Screenshots you want to paste into a chat / search

### Build

```bash
cargo install --path /path/to/bilitools-cli --features ocr
# or, from the repo:
cargo build --release --features ocr
```

The `ocr` feature pulls in:

- `ocr-rs` (2.2.2) — Rust wrapper around the **MNN** inference engine
- `imageproc` — needed because `ocr-rs::TextBox` exposes
  `imageproc::rect::Rect` as a public field
- MNN C++ runtime — compiled statically by `ocr-rs`'s build.rs (needs
  `libclang-dev` to run bindgen at build time)

Build-time prerequisite (one time):

```bash
sudo apt install libclang-dev clang   # Debian/Ubuntu
brew install llvm                    # macOS
choco install llvm                   # Windows
```

### Install OCR models (one time)

Three MNN model files, **10.4 MB total**:

```bash
mkdir -p models/ocr-fast
# Recommended FP16 (fastest, ~10 MB):
#   PP-OCRv5_mobile_det_fp16.mnn
#   PP-OCRv5_mobile_rec_fp16.mnn
#   ppocr_keys_v5.txt
#
# These are NOT committed to git. Download from the project's
# GitHub Release tarball, or place them manually.

# Or override the location with an env var:
export BILITOOLS_OCR_MODEL_DIR=/path/to/models/ocr-fast
```

Search order at runtime:

1. `$BILITOOLS_OCR_MODEL_DIR`
2. `<exe-dir>/models/ocr-fast/`
3. `<exe-dir>/`
4. `./models/ocr-fast/`
5. `./`

### Usage

```bash
# Image OCR
bilitools ocr screenshot.png
bilitools --json ocr screenshot.png -o ./out

# Video OCR (extracts frames via ffmpeg, then OCRs each)
bilitools ocr video.mp4 --video --interval 1.0
bilitools ocr video.mp4 --video --interval 30 --max-frames 5
bilitools --json ocr video.mp4 --video -o ./out

# B 站 BV workflow: download the video first, then OCR it
bilitools download submit BV1XBRuBSEd7 -o ./bv
bilitools download run <task_id>
bilitools ocr ./bv/<title>-<cid>.mp4 --video --interval 30 -o ./ocr
```

### Flags (ocr subcommand)

| Flag | Default | Description |
|------|---------|-------------|
| `INPUT` | — | Image file, or (with `--video`) a local video file |
| `--video` | off | Treat `INPUT` as a video and run ffmpeg frame extraction first |
| `--interval <seconds>` | `1.0` | Seconds between sampled frames (video mode) |
| `--max-frames <N>` | `200` | Hard cap on frames OCR'd from a video |
| `--min-conf <0..1>` | `0.45` | Drop detections below this confidence |
| `-o, --output-dir <path>` | `./ocr_out/<unix-ts>/` | Where to write `ocr.json` and (optionally) frames |
| `--keep-frames` | off | Keep extracted frames on disk after OCR (default: delete) |

### Output

A single `ocr.json` written to `-o`:

```json
{
  "mode": "video",
  "input": "video.mp4",
  "video_path": "/abs/path/video.mp4",
  "frames_processed": 5,
  "interval_sec": 30.0,
  "detections": [
    {
      "t_sec": 0.0,
      "text": "桂林雨中游湖",
      "confidence": 1.0,
      "bbox": [[775, 877], [1143, 877], [1143, 959], [775, 959]]
    },
    {
      "t_sec": 30.0,
      "text": "遇龙河竹筏游大暴雨淋成落汤鸡",
      "confidence": 1.0,
      "bbox": [[449, 893], [1473, 893], ...]
    }
  ]
}
```

Image mode is the same shape minus the `t_sec` / `video_path` fields.

### Performance (CPU)

PP-OCRv5 mobile on a 1920×1080 frame: **~1.5 s/frame** on the WSL sandbox
(12 vCPU). A 2-hour video at `--interval 1.0` = 7200 frames = ~3 hours wall
time — tune `--interval` to taste. The engine caps at 3 threads by default;
override with `BILITOOLS_OCR_THREADS`.

### Common errors

| Error message | Fix |
|---------------|-----|
| `bilitools: 'ocr' was not a subcommand` | binary was built without `--features ocr` |
| `OCR model files not found` | install the three MNN files per above |
| `ffmpeg not found in PATH` | `sudo apt install ffmpeg` |
| `libclang not found` at build time | `sudo apt install libclang-dev clang` |
| `unresolved import \`ocr_rs\`` | rebuild with `cargo build --release --features ocr` |

### Known limitations (rpic-validated)

- **Small text** (< ~16 px equivalent) — high miss rate. E.g. the small
  "bilibili" watermark in the top-right of B 站 videos usually gets merged
  into "blbi" or dropped. **Main titles and chapter cards work great**
  (confidence 0.98–1.00 on the v1 风景旅行收藏家 video we tested).
- **Extreme font effects** (neon, shadow, glow) — confidence drops but
  text is still usually correct
- **Reflections / glare** — 100% miss

## Data Locations

| What | Path |
|------|------|
| SQLite (cookies, tasks, settings) | `$XDG_DATA_HOME/com.btjawa.bilitools/Storage/storage.db` |
| Override | `BILITOOLS_DATA_DIR=/tmp/foo` |
| Config TOML | `<data_dir>/config.toml` |
| Cookies (within DB) | `SELECT name, value FROM cookies WHERE name='SESSDATA'` |
| Task logs (within DB) | `SELECT * FROM task_events WHERE task_id = '...'` |
| Sidecar overrides | `BILITOOLS_SIDECAR_ARIA2C=/path/...` etc. |
| Log level | `--log-level trace|debug|info|warn|error` (default `info`) |

## Quick Diagnostic Commands

```bash
# Login state
bilitools --json auth status | jq .data

# Health check
bilitools doctor

# Force-refresh fingerprint cookies
bilitools init

# What is in HEADERS right now?
bilitools --json auth status | jq '.data.cookies'
```

## When This Skill Should NOT Be Used

- **Bulk scraping across many accounts** — B 站 will rate-limit. Use the official API
  or a higher-throughput tool.
- **Re-encoding video** — `bilitools download` does `-c copy` (no re-encoding). For
  re-encoding (smaller files, different codec), post-process with ffmpeg directly.
- **Live streaming downloads** — `live` is in the search type list, but the live stream
  download path is not part of this CLI. Use a different tool.
- **Real-time / streaming ASR** — the `audio --transcribe` flow is batch-only (you
  wait for the whole audio to finish). For live captions or <100 ms latency,
  use a streaming ASR tool directly.

## Source

- GitHub: <https://github.com/nekobaimeow/bilitools-cli>
- Upstream (GUI original): <https://github.com/btjawa/BiliTools>
- License: GPL-3.0-or-later (inherited from BiliTools)
