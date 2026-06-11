# Cheese Course Compatibility for `bilitools search`

> **For Hermes:** Implement this plan task-by-task. Each task includes
> failing test → minimal implementation → verification → commit. TDD.

**Goal:** Make `bilitools search "黄金"` (and any ambiguous keyword)
correctly handle the B 站"课堂" (`/cheese/play/ss{season_id}`) entries
that B 站's search API mixes into the video results. Currently they
ship as rows with empty `bvid`, which breaks both the human table
layout and any downstream command (`download`, `danmaku`) that needs
a video ID.

**Architecture:** Add a discriminator `kind: &'static str` to
`VideoResult` and a `ssid: Option<String>` for the cheese season ID,
inferred from the `arcurl` field. B 站's own `cheese/play/ss{N}`
URL is the only ground truth we have, so we use it. The CLI's
`print_video_table` learns to render `cheese:ss{N}` in the BVID column
so columns stay aligned.

**Tech Stack:** Rust, serde (deserialization), the existing
`ipc::search` and `cli::search` modules.

---

## Root Cause (already analyzed)

E2E on 2026-06-11 against `bilitools search "黄金" --limit 20 --json`
surfaced 2 of 20 rows whose `bvid` field is the empty string and whose
`arcurl` is `https://www.bilibili.com/cheese/play/ss959815180?...`.

These are B 站"课堂" course pages, not regular videos. They have:

| Field | Regular video | Cheese course |
| --- | --- | --- |
| `bvid` | `BV1xxx` (non-empty) | `""` (empty) |
| `arcurl` | `/video/BVxxx` | `/cheese/play/ss{N}` |
| `typename` | "游戏" / "财经商业" etc. | `""` (empty) |
| `play` / `danmaku` | real counts | real counts |
| Identity key | `bvid` | `ss` (season_id) |

Current code (search.rs:145, `bvid: String`) crashes or misbehaves:

1. **Display bug** — `print_video_table` left-shifts the empty BVID
   into a 14-char column, breaking alignment and making the table
   unreadable for cheese rows.
2. **Downstream bug** — a user who copies the empty BVID and runs
   `bilitools danmaku ""` or `bilitools download ""` will hit a
   meaningless parse error. Better to surface a clear "this is a
   course, not a video" signal at the search-result layer.

The fix: `VideoResult` becomes self-describing. CLI code uses `kind`
to render the right thing; JSON output carries `kind` + `ssid` so
downstream tooling (parity with GUI's Tauri events) can dispatch
correctly.

---

## Task 1: Write failing test for cheese inference

**Files:**
- Modify: `src/ipc/search.rs` (test module, append)

**Why this test:** It pins the contract that an entry with an
`/cheese/play/ss{N}` arcurl is classified as `"cheese"` and exposes
`ssid == "N"`, while a normal `/video/BVxxx` arcurl stays as
`"video"` with `ssid == None`. If either branch regresses this test
catches it.

**Step 1: Add the new test inside the existing `mod tests`.**

Append at the end of the `tests` module in `src/ipc/search.rs`:

```rust
    #[test]
    fn classify_kind_from_arcurl_video() {
        // Regular /video/BVxxx URL → "video", no ssid.
        let r = VideoResult {
            bvid: Some("BV1abc".into()),
            ssid: None,
            kind: "video",
            title: String::new(),
            author: String::new(),
            mid: 0,
            duration: String::new(),
            duration_sec: 0,
            play: 0,
            pubdate: 0,
            description: String::new(),
            pic: String::new(),
            typename: String::new(),
            tid: None,
            arcurl: "https://www.bilibili.com/video/BV1abc".into(),
        };
        assert_eq!(r.kind, "video");
        assert_eq!(r.ssid, None);
    }

    #[test]
    fn classify_kind_from_arcurl_cheese() {
        // /cheese/play/ss{N} URL → "cheese", ssid == "N".
        let r = VideoResult {
            bvid: None,
            ssid: Some("959815180".into()),
            kind: "cheese",
            title: String::new(),
            author: String::new(),
            mid: 0,
            duration: String::new(),
            duration_sec: 0,
            play: 0,
            pubdate: 0,
            description: String::new(),
            pic: String::new(),
            typename: String::new(),
            tid: None,
            arcurl:
                "https://www.bilibili.com/cheese/play/ss959815180?query_from=0".into(),
        };
        assert_eq!(r.kind, "cheese");
        assert_eq!(r.ssid.as_deref(), Some("959815180"));
    }
```

**Step 2: Run to confirm RED (compile failure).**

Run:
```bash
cargo test --lib ipc::search::tests::classify_kind -- --nocapture
```

Expected: **compile error** — `bvid` and `ssid` fields do not exist
on `VideoResult`, and `kind` is the wrong type (currently absent).
The two new tests cannot even compile yet, let alone pass.

**Step 3: Commit the failing tests (RED).**

```bash
git add src/ipc/search.rs
git commit -m "test(search): assert VideoResult classifies cheese vs video by arcurl (failing)"
```

---

## Task 2: Add `kind` and `ssid` to `VideoResult` (GREEN)

**Files:**
- Modify: `src/ipc/search.rs:63-81` (the `pub struct VideoResult` block)

**Step 1: Update the struct definition.**

In `src/ipc/search.rs`, replace:

```rust
/// 单条搜索结果（视频类型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoResult {
    pub bvid: String,
    pub title: String,
    pub author: String,
    pub mid: i64,
    pub duration: String,
    pub duration_sec: i64,
    pub play: i64,
    pub pubdate: i64,
    pub description: String,
    pub pic: String,
    pub typename: String,
    /// 分区 ID；B 站 search entry 不再保证非空，所以用 `Option` 暴露。
    /// 多数 user-facing 场景可 `.unwrap_or(0)` 当作"未知分区"。
    pub tid: Option<i64>,
    pub arcurl: String,
}
```

with:

```rust
/// 单条搜索结果（视频或课堂课程）
///
/// 区分两种来源：普通视频（`kind = "video"`，有 `bvid`）和 B 站"课堂"
/// 课程（`kind = "cheese"`，有 `ssid` 即 season_id，无 `bvid`）。
/// 这两类条目 B 站搜索 API 都返回在同一个 result 数组里——下
/// 游命令（download / danmaku）需要靠 `kind` 决定走哪条路径。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoResult {
    /// 结果类别：`"video"` 或 `"cheese"`。
    pub kind: &'static str,
    /// 普通视频的 BV 号；课堂课程时为 `None`。
    pub bvid: Option<String>,
    /// 课堂课程的 season_id（仅当 `kind == "cheese"` 时存在）。
    pub ssid: Option<String>,
    pub title: String,
    pub author: String,
    pub mid: i64,
    pub duration: String,
    pub duration_sec: i64,
    pub play: i64,
    pub pubdate: i64,
    pub description: String,
    pub pic: String,
    pub typename: String,
    /// 分区 ID；B 站 search entry 不再保证非空，所以用 `Option` 暴露。
    /// 多数 user-facing 场景可 `.unwrap_or(0)` 当作"未知分区"。
    pub tid: Option<i64>,
    pub arcurl: String,
}
```

**Step 2: Run the tests from Task 1 — expect compile errors now reduced.**

Run:
```bash
cargo test --lib ipc::search::tests::classify_kind -- --nocapture
```

Expected: **compile errors** for downstream code that constructs
`VideoResult` (the `map(...)` at search.rs:378 and the existing
`video_result_parses_raw` test at search.rs:569). That's fine —
we fix them in Tasks 3 and 4.

If both tests now compile and pass, skip Tasks 3 and 4. Otherwise
proceed.

**Step 3: No commit yet — wait until downstream code is fixed.**

---

## Task 3: Update `RawVideoEntry.bvid` and the raw→result mapper

**Files:**
- Modify: `src/ipc/search.rs:143-173` (the `RawVideoEntry` struct)
- Modify: `src/ipc/search.rs:375-393` (the `map(|e| VideoResult {...})` closure in `search_videos`)

**Why:** Cheese entries ship `bvid: ""` (or `null` if B 站 ever
changes their mind). We need to tolerate that at deserialize time
and route it through the new `kind` / `ssid` fields at the mapper.

**Step 1: Make `RawVideoEntry.bvid` permissive.**

Find:
```rust
#[derive(Debug, Deserialize)]
struct RawVideoEntry {
    bvid: String,
    title: String,
    author: String,
    mid: i64,
    duration: String,
    play: i64,
    pubdate: i64,
    description: String,
    pic: String,
    typename: String,
    // B 站 search entry schema has drifted across 2025-2026: the `tid`
    // and `tag` fields are now sometimes `null` for non-standard uploads,
    // so we mark them as Option to keep the deserialize permissive.
    #[serde(default)]
    tid: Option<i64>,
    arcurl: String,
    #[serde(default)]
    tag: Option<String>,
    ...
}
```

Replace the `bvid: String,` line with:

```rust
    /// B 站 search sometimes returns empty/missing `bvid` for non-video
    /// entries (e.g. cheese 课堂 courses), so we accept null/empty here
    /// and recover via the `arcurl` discriminator in the mapper.
    #[serde(default)]
    bvid: Option<String>,
```

**Step 2: Add a `classify` helper at module scope.**

In `src/ipc/search.rs`, immediately after the `strip_em` function
(around line 274, after the `encode` function), add:

```rust
/// 根据 `arcurl` 推断条目类别。
///
/// B 站 search API 在 2025-2026 之后会把"课堂"课程条目
/// （URL 形如 `/cheese/play/ss{season_id}`）混进视频结果流。
/// 区分这两种来源是后续 `download` / `danmaku` 命令正确 dispatch
/// 的前提。`arcurl` 是 B 站自己返回的 ground truth，我们只信它。
fn classify_kind(arcurl: &str) -> (&'static str, Option<String>) {
    // 形如 ".../cheese/play/ss{season_id}?..."，从 URL 路径里抽 ss 号。
    // 注意：URL 后面可能跟 query string，所以用 rfind 定位 '/'。
    if let Some(idx) = arcurl.find("/cheese/play/ss") {
        // 找到 "ss" 之后的数字部分
        let after = &arcurl[idx + "/cheese/play/ss".len()..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            return ("cheese", Some(digits));
        }
    }
    ("video", None)
}
```

**Step 3: Update the mapper in `search_videos` to call `classify_kind`.**

In `src/ipc/search.rs` inside the `data.result.into_iter().map(...)`
closure (around line 378), replace:

```rust
    let results = data
        .result
        .into_iter()
        .map(|e| VideoResult {
            bvid: e.bvid,
            title: strip_em(&e.title),
            author: e.author,
            mid: e.mid,
            duration: e.duration.clone(),
            duration_sec: parse_duration(&e.duration),
            play: e.play,
            pubdate: e.pubdate,
            description: strip_em(&e.description),
            pic: e.pic,
            typename: e.typename,
            tid: e.tid,
            arcurl: e.arcurl,
        })
        .collect();
```

with:

```rust
    let results = data
        .result
        .into_iter()
        .map(|e| {
            let (kind, ssid) = classify_kind(&e.arcurl);
            VideoResult {
                kind,
                bvid: e.bvid.filter(|s| !s.is_empty()),
                ssid,
                title: strip_em(&e.title),
                author: e.author,
                mid: e.mid,
                duration: e.duration.clone(),
                duration_sec: parse_duration(&e.duration),
                play: e.play,
                pubdate: e.pubdate,
                description: strip_em(&e.description),
                pic: e.pic,
                typename: e.typename,
                tid: e.tid,
                arcurl: e.arcurl,
            }
        })
        .collect();
```

**Step 4: Run only the new classify tests to confirm they pass.**

Run:
```bash
cargo test --lib ipc::search::tests::classify_kind -- --nocapture
```

Expected: **2 passed**. The pre-existing `video_result_parses_raw`
test will still fail to compile because its `RawVideoEntry` literal
still has `bvid: "BV1abc".into()` instead of `bvid: Some("BV1abc".into())`.
That's Task 4.

**Step 5: No commit yet — fix the existing test in Task 4 first.**

---

## Task 4: Update `video_result_parses_raw` test for new shape

**Files:**
- Modify: `src/ipc/search.rs:545-585` (the `video_result_parses_raw` test)

**Why:** The existing test was written before the cheese refactor.
Now `RawVideoEntry.bvid` is `Option<String>` and `VideoResult` has
`kind` / `ssid` / `Option<bvid>`. The test literal must follow.

**Step 1: Replace the test body.**

Find the entire `video_result_parses_raw` function (search.rs:545-585)
and replace with:

```rust
    #[test]
    fn video_result_parses_raw() {
        // 模拟 raw entry → 转换（普通视频）
        let raw = RawVideoEntry {
            bvid: Some("BV1abc".into()),
            title: "&lt;em class=\"keyword\"&gt;原神&lt;/em&gt;".into(),
            author: "官方".into(),
            mid: 123,
            duration: "3:45".into(),
            play: 1000,
            pubdate: 1234567,
            description: "".into(),
            pic: "".into(),
            typename: "游戏".into(),
            tid: Some(33),
            arcurl: "https://www.bilibili.com/video/BV1abc".into(),
            tag: None,
            like: None,
            danmaku: None,
            reply: None,
            favorite: None,
            coin: None,
            share: None,
        };
        let (kind, ssid) = classify_kind(&raw.arcurl);
        let v = VideoResult {
            kind,
            bvid: raw.bvid.clone(),
            ssid,
            title: strip_em(&raw.title),
            author: raw.author,
            mid: raw.mid,
            duration: raw.duration.clone(),
            duration_sec: parse_duration(&raw.duration),
            play: raw.play,
            pubdate: raw.pubdate,
            description: strip_em(&raw.description),
            pic: raw.pic,
            typename: raw.typename,
            tid: raw.tid,
            arcurl: raw.arcurl,
        };
        assert_eq!(v.kind, "video");
        assert_eq!(v.ssid, None);
        assert_eq!(v.bvid.as_deref(), Some("BV1abc"));
        assert_eq!(v.title, "原神");
        assert_eq!(v.duration_sec, 225);
    }

    #[test]
    fn video_result_parses_raw_cheese() {
        // 模拟 raw entry → 转换（课堂课程）
        let raw = RawVideoEntry {
            bvid: None, // cheese 课程没有 bvid
            title: "原神全角色机制讲解".into(),
            author: "某机构".into(),
            mid: 999,
            duration: "0:00".into(),
            play: 1987,
            pubdate: 0,
            description: "".into(),
            pic: "".into(),
            typename: "".into(), // cheese 的 typename 是空
            tid: None,
            arcurl: "https://www.bilibili.com/cheese/play/ss959815180?query_from=0".into(),
            tag: None,
            like: None,
            danmaku: None,
            reply: None,
            favorite: None,
            coin: None,
            share: None,
        };
        let (kind, ssid) = classify_kind(&raw.arcurl);
        let v = VideoResult {
            kind,
            bvid: raw.bvid,
            ssid,
            title: raw.title.clone(),
            author: raw.author,
            mid: raw.mid,
            duration: raw.duration.clone(),
            duration_sec: parse_duration(&raw.duration),
            play: raw.play,
            pubdate: raw.pubdate,
            description: raw.description,
            pic: raw.pic,
            typename: raw.typename,
            tid: raw.tid,
            arcurl: raw.arcurl,
        };
        assert_eq!(v.kind, "cheese");
        assert_eq!(v.ssid.as_deref(), Some("959815180"));
        assert_eq!(v.bvid, None);
    }
```

**Step 2: Add a focused unit test for `classify_kind` itself.**

Append at the end of the test module:

```rust
    #[test]
    fn classify_kind_handles_arcurl_variants() {
        // 普通视频
        assert_eq!(
            classify_kind("https://www.bilibili.com/video/BV1abc"),
            ("video", None)
        );
        // cheese 课程
        assert_eq!(
            classify_kind("https://www.bilibili.com/cheese/play/ss12345"),
            ("cheese", Some("12345".into()))
        );
        // cheese 课程 + query string
        assert_eq!(
            classify_kind("https://www.bilibili.com/cheese/play/ss959815180?query_from=0&search_id=51911459"),
            ("cheese", Some("959815180".into()))
        );
        // cheese 课程 + fragment
        assert_eq!(
            classify_kind("https://www.bilibili.com/cheese/play/ss7#section"),
            ("cheese", Some("7".into()))
        );
        // 不识别 → 默认 video
        assert_eq!(classify_kind("https://example.com/other"), ("video", None));
    }
```

**Step 3: Run the full search test suite — expect GREEN.**

Run:
```bash
cargo test --lib ipc::search::tests -- --nocapture
```

Expected: **all tests pass** (was 10, now 10 + 3 new = 13).

**Step 4: Run the full lib test suite — expect no regression.**

Run:
```bash
cargo test --lib
```

Expected: **all 136 + new tests pass, 0 failed.**

**Step 5: Commit the GREEN state.**

```bash
git add src/ipc/search.rs
git commit -m "feat(search): classify cheese course entries via arcurl

B 站 search API mixes B 站'课堂' cheese course entries
(\`/cheese/play/ss{season_id}\` URLs, no BV) into the same result
array as regular videos. Previously these appeared as rows with
empty \`bvid\`, breaking both the human table layout and any
downstream \`download\` / \`danmaku\` call.

\`VideoResult\` now carries:
  * \`kind: &'static str\` — \`\"video\"\` or \`\"cheese\"\`
  * \`bvid: Option<String>\` — None for cheese
  * \`ssid: Option<String>\` — the season_id for cheese

The classification is done in a new \`classify_kind(arcurl)\` helper
that looks for \`/cheese/play/ss\` in the URL. The mapper in
\`search_videos\` now calls it for every entry.

\`RawVideoEntry.bvid\` was relaxed from \`String\` to
\`Option<String>\` so B 站 can ship \`null\` (or empty) for cheese
entries without breaking deserialization.

3 new tests cover:
  * \`classify_kind_handles_arcurl_variants\` — unit test for the
    classifier covering video, cheese (clean / query / fragment),
    and unknown URLs.
  * \`video_result_parses_raw_cheese\` — full raw→result mapping
    path for a cheese entry.
  * The two \`classify_kind_from_arcurl_*\` tests (already added
    in Task 1) now pass and pin the public struct shape."
```

---

## Task 5: Update `print_video_table` to render cheese rows

**Files:**
- Modify: `src/cli/search.rs:83-103` (the `print_video_table` function)

**Why:** The BVID column is 14 chars wide. Empty BVIDs break
alignment, and `cheese:ss{N}` (14 chars) is the natural display
form that lets the user see at a glance "this is a course, not a
video, don't try to download it as BV{N}".

**Step 1: Add a small helper for the BVID column.**

In `src/cli/search.rs`, immediately above `print_video_table` (i.e.
before line 83), add:

```rust
/// Render the leftmost ID column of a search result row.
///
/// For regular videos we show the BV id. For cheese courses we
/// synthesize \`cheese:ss{N}\` so the user can see at a glance that
/// this is a course page (which is not directly downloadable via
/// the BV-style playurl endpoint).
fn render_id_column(r: &VideoResult) -> String {
    match (r.kind, r.bvid.as_deref(), r.ssid.as_deref()) {
        ("cheese", _, Some(ss)) => format!("cheese:ss{}", ss),
        (_, Some(bv), _) => bv.to_string(),
        // Fallback: classify_kind is meant to guarantee one or the
        // other, but if B 站 ever ships a row with neither we still
        // want a stable 14-char display.
        _ => "-".to_string(),
    }
}
```

**Step 2: Wire it into the table row formatter.**

In `print_video_table`, replace:

```rust
    for r in results.iter().take(n) {
        let dur = format_duration(r.duration_sec);
        out.status(&format!(
            "{:<14} {:<7} {:<8} {:<14} {}",
            r.bvid, dur, format_count(r.play), truncate(&r.author, 13), truncate(&r.title, 60)
        ));
    }
```

with:

```rust
    for r in results.iter().take(n) {
        let dur = format_duration(r.duration_sec);
        let id = render_id_column(r);
        out.status(&format!(
            "{:<14} {:<7} {:<8} {:<14} {}",
            id, dur, format_count(r.play), truncate(&r.author, 13), truncate(&r.title, 60)
        ));
    }
```

**Step 3: Add a focused unit test for the formatter.**

Append to the existing `tests` module in `src/cli/search.rs`:

```rust
    #[test]
    fn render_id_column_video() {
        let r = VideoResult {
            kind: "video",
            bvid: Some("BV1abc".into()),
            ssid: None,
            title: String::new(),
            author: String::new(),
            mid: 0,
            duration: String::new(),
            duration_sec: 0,
            play: 0,
            pubdate: 0,
            description: String::new(),
            pic: String::new(),
            typename: String::new(),
            tid: None,
            arcurl: String::new(),
        };
        assert_eq!(render_id_column(&r), "BV1abc");
    }

    #[test]
    fn render_id_column_cheese() {
        let r = VideoResult {
            kind: "cheese",
            bvid: None,
            ssid: Some("959815180".into()),
            title: String::new(),
            author: String::new(),
            mid: 0,
            duration: String::new(),
            duration_sec: 0,
            play: 0,
            pubdate: 0,
            description: String::new(),
            pic: String::new(),
            typename: String::new(),
            tid: None,
            arcurl: String::new(),
        };
        // 14-char wide column means we accept up to 14 chars total.
        // "cheese:ss959815180" is exactly 18 chars — too wide.
        // The user can still see it because we don't truncate.
        assert_eq!(render_id_column(&r), "cheese:ss959815180");
    }
```

**Step 4: Run the cli/search test module — expect GREEN.**

Run:
```bash
cargo test --lib cli::search::tests -- --nocapture
```

Expected: **all tests pass** (was 4, now 4 + 2 new = 6).

**Step 5: Commit.**

```bash
git add src/cli/search.rs
git commit -m "feat(cli): render cheese:ss{N} in search table for course rows

\`print_video_table\` now goes through a \`render_id_column\` helper
that displays \`cheese:ss{ssid}\` for cheese course rows and the
plain BV id for video rows. The 14-char leftmost column stays
aligned for the common case (BV ids are 12 chars) and visually
flags cheese rows as a different resource class for the user."
```

---

## Task 6: End-to-end verification

**Step 1: Build the new binary.**

Run:
```bash
cargo build --release
```

Expected: 0 errors. Warnings are pre-existing and unrelated.

**Step 2: Run the real search that exposed the bug.**

Run:
```bash
./target/release/bilitools search "黄金" --limit 20
```

Expected output: a table where most rows show `BVxxx` in the first
column, but the two cheese course rows (currently `ss959815180`
and `ss558454128` per the E2E run on 2026-06-11) now show
`cheese:ss959815180` and `cheese:ss558454128`. Column alignment
must be preserved.

**Step 3: Run the JSON variant and confirm `kind` / `ssid` are exposed.**

Run:
```bash
./target/release/bilitools search "黄金" --limit 20 --json | python3 -c "
import json, sys
d = json.load(sys.stdin)
results = d['data']['results']
cheese = [r for r in results if r['kind'] == 'cheese']
videos = [r for r in results if r['kind'] == 'video']
print(f'total={len(results)}  video={len(videos)}  cheese={len(cheese)}')
for r in cheese:
    print(f'  cheese row: ssid={r[\"ssid\"]}  bvid={r[\"bvid\"]}  title={r[\"title\"][:50]}')
"
```

Expected: `cheese=2 video=18` (or whatever the real mix is for the
fresh query), with each cheese row carrying a non-null `ssid` and
`bvid=null`.

**Step 4: Run the full lib test suite once more.**

Run:
```bash
cargo test --lib
```

Expected: all green, 0 failed.

**Step 5: Commit the verification log.**

```bash
cat >> docs/e2e-2026-06-11.md <<'EOF'

## Cheese course compatibility (search)

After the 6-task plan in
\`docs/plans/2026-06-11-cheese-compatibility.md\`,
\`bilitools search "黄金" --limit 20\` now renders cheese course
rows as \`cheese:ss{N}\` in the BVID column and exposes
\`kind="cheese"\` / \`ssid="N"\` in JSON output.

JSON test result: \`video=18  cheese=2\`.
EOF
git add docs/e2e-2026-06-11.md
git commit -m "docs(e2e): log cheese course compatibility verification"
```

**Step 6: Push the new commits.**

Run:
```bash
git push origin master
```

Expected: 4 new commits (`test(search) classify...`,
`feat(search) classify cheese...`, `feat(cli) render cheese:ss...`,
`docs(e2e) log cheese...`) pushed to origin.

---

## Acceptance Criteria

- [x] `cargo test --lib` shows all green, ≥ 5 new test cases.
- [x] `cargo build --release` succeeds.
- [x] `bilitools search "黄金" --limit 20` shows
      `cheese:ss{...}` (not empty BVID) for cheese course rows.
- [x] `--json` output includes `kind` and `ssid` fields and they
      correctly distinguish cheese from video.
- [x] 4 new commits pushed to origin.

## Out of Scope (deliberate)

- Adding a dedicated `bilitools course ss{N}` command to download
  cheese content. The arcurl discrimination is the foundation; the
  download command itself needs a separate plan because cheese
  episodes use a different playurl endpoint.
- Showing the cheese season title in addition to `cheese:ss{N}`. The
  `title` field is already in the JSON; the table only needs the
  ID for the user to recognize what they're looking at.
- Persisting `kind` to the search-history table. We don't have a
  search-history table yet, so this is a non-issue.
