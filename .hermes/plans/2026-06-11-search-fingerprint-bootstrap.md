# Fix `bilitools search` HTTP 412: Bootstrap B 站 风控 Fingerprint at Startup

> **For Hermes:** Implement this plan task-by-task. Each task includes failing
> test → minimal implementation → verification. Commit after every task.

**Goal:** Make `bilitools search "原神"` succeed end-to-end. Currently returns
HTTP 412 because the B 站 风控 layer requires `buvid3`, `bili_ticket`, and
`_uuid` cookies in addition to the login `SESSDATA`.

**Architecture:** Move the three finger-print-fetch calls that currently live
inside the `init` subcommand into `AppContext::build()`. This way every CLI
command that touches `context::ctx()` (i.e. every command) gets a fully
bootstrapped `HEADERS` global state. The `init` subcommand becomes a no-op
alias for backwards compatibility.

**Tech Stack:** Rust, tokio, reqwest, sqlx (SQLite), the existing
`ipc::login` module.

---

## Root Cause (already analyzed)

| Component | Status at startup | Effect |
| --- | --- | --- |
| `cookies::load()` → `HEADERS.refresh()` | ✅ Works (`init_client_inner:139`) | SESSDATA, bili_jct, etc. present |
| `buvid3` / `buvid4` (web cookies) | ❌ Missing | Required by `x/web-interface/nav` |
| `bili_ticket` (HSM-signed ticket) | ❌ Missing | Required by nav 风控 |
| `_uuid` (INFOC fingerprint) | ❌ Missing | Required by nav 风控 |

Without the bottom three, `get_wbi_keys()` at `shared.rs:266` calls
`x/web-interface/nav` → 412 → `WBI_KEYS` stays `None` → `wbi_sign` errors out
→ `search::get_json` falls back to unsigned query → 412 again.

The fix: in `AppContext::build()`, after `db::init()`, call
`login::get_buvid()`, `login::get_bili_ticket()`, `login::get_uuid()` so
`HEADERS` ends up with all 风控 cookies before any user-facing HTTP request
runs.

---

## Task 1: Write a failing unit test for `AppContext::build` fingerprint bootstrap

**Files:**
- Modify: `src/context.rs` (test module at bottom)
- Test: `src/context.rs` (`#[cfg(test)] mod tests`)

**Why this test:** It calls `ctx().await` after we wire in the bootstrap and
asserts that `HEADERS.cookie()` contains all three marker strings
(`buvid3=`, `bili_ticket=`, `_uuid=`). If any one is missing the test fails
with a clear error message telling us which fingerprint is not bootstrapped.

**Step 1: Read the current test module to know its structure.**

```bash
sed -n '111,119p' src/context.rs
```

Expected: a `mod tests` block with one test `paths_in_context_consistent`.

**Step 2: Add the new failing test inside the existing `mod tests`.**

Append after the existing test in `src/context.rs`:

```rust
#[tokio::test]
async fn context_bootstraps_fingerprint_cookies() {
    // Reset state by calling ctx() (idempotent; if already built it returns
    // the same Arc without rebuilding). That's fine for this test because
    // we just want to assert that whoever built it, the fingerprint is
    // present.
    let _ = ctx().await.expect("ctx should build");
    let cookie = crate::ipc::shared::HEADERS.cookie().await;
    assert!(
        cookie.contains("buvid3="),
        "buvid3 missing from HEADERS — get_buvid() was not called during build; \
         cookie={cookie}"
    );
    assert!(
        cookie.contains("bili_ticket="),
        "bili_ticket missing from HEADERS — get_bili_ticket() was not called \
         during build; cookie={cookie}"
    );
    assert!(
        cookie.contains("_uuid="),
        "_uuid missing from HEADERS — get_uuid() was not called during build; \
         cookie={cookie}"
    );
}
```

**Step 3: Run the test to confirm it FAILS (RED).**

Run:
```bash
cargo test --lib context::tests::context_bootstraps_fingerprint_cookies -- --nocapture
```

Expected: **FAIL** with one of the three assertion messages
(`buvid3 missing ... cookie=...`). The cookie value will be SESSDATA + maybe
a few persistent cookies but no `buvid3=`.

If the test passes, something already booted fingerprint and Task 1 has
nothing to fix — go straight to Task 3.

**Step 4: Commit the failing test (RED state).**

```bash
git add src/context.rs
git commit -m "test(context): assert HEADERS has buvid3/bili_ticket/_uuid after build (failing)"
```

---

## Task 2: Wire `login::get_buvid()`, `get_bili_ticket()`, `get_uuid()` into `AppContext::build()`

**Files:**
- Modify: `src/context.rs:54-67` (the `AppContext::build` body, right after `db::init()`)

**Why this implementation:** It's the minimum change to make HEADERS have
all three cookies at startup. We reuse the exact same functions that
`cmd_init` already calls (zero new logic, zero new code paths — just moved
into the always-on boot path).

**Step 1: Add the `login` import.**

At the top of `src/context.rs`, change:

```rust
use crate::ipc::shared::{init_client, init_client_no_proxy, set_proxy};
use crate::ipc::storage::{config, db};
```

to:

```rust
use crate::ipc::login;
use crate::ipc::shared::{init_client, init_client_no_proxy, set_proxy};
use crate::ipc::storage::{config, db};
```

**Step 2: Insert the three fingerprint calls after `db::init()`.**

In `src/context.rs` inside `AppContext::build()`, find:

```rust
        // Initialize DB (idempotent).
        db::init().await?;

        // Load settings, then construct HTTP clients with the right proxy.
        let settings = config::read().await;
```

Replace with:

```rust
        // Initialize DB (idempotent).
        db::init().await?;

        // Bootstrap B 站 风控 fingerprint cookies. These are NOT persisted
        // (B 站 rotates them on a session scale and rejects requests that
        // ship a SESSDATA without the matching buvid3 / bili_ticket / _uuid).
        // We call them once at startup so every subsequent request — search,
        // download, danmaku — has a fully-formed Cookie header.
        //
        // If any of these fail we propagate the error to the caller. The
        // fingerprint APIs themselves are public; failures here usually mean
        // a network/proxy problem the user wants to see, not silently boot
        // with a broken cookie header.
        login::get_buvid().await?;
        login::get_bili_ticket().await?;
        login::get_uuid().await?;

        // Load settings, then construct HTTP clients with the right proxy.
        let settings = config::read().await;
```

**Step 3: Run the test from Task 1 — expect GREEN.**

Run:
```bash
cargo test --lib context::tests::context_bootstraps_fingerprint_cookies -- --nocapture
```

Expected: **PASS** (1 passed). If it still fails, check that the network is
reachable and that `cookies::load()` isn't returning stale data — run with
`RUST_LOG=bilitools=debug cargo test ...` and look for the
`HEADERS refreshed` trace line which now should report a long cookie
including `buvid3=...; bili_ticket=...; _uuid=...`.

**Step 4: Run the full test suite — expect 138 passing.**

Run:
```bash
cargo test --lib
```

Expected: **138 passed** (was 137 + 1 new test from Task 1). If any pre-existing
test breaks, investigate before committing.

**Step 5: Commit (GREEN state).**

```bash
git add src/context.rs
git commit -m "fix(context): bootstrap buvid3/bili_ticket/_uuid in AppContext::build

B 站's nav API (and by extension WBI-protected search) returns HTTP 412
when the request ships a SESSDATA without the matching session-scoped
fingerprint cookies. Previously these were only fetched by the explicit
`bilitools init` subcommand, so any other command (search, danmaku,
download) hit nav with an incomplete cookie jar and got rejected.

Now `AppContext::build()` — which runs at the top of every CLI command —
calls login::get_buvid() / get_bili_ticket() / get_uuid() so the global
HEADERS are fully populated before any user-facing request fires."
```

---

## Task 3: Make `init` subcommand a no-op alias for backwards compatibility

**Files:**
- Modify: `src/main.rs:79-86` (`cmd_init` body)

**Why:** The `init` command used to be the only place fingerprint cookies
were fetched. With Task 2 the fingerprint is already loaded by
`context::ctx()` at the start of every command, including `init` itself.
The user can still type `bilitools init` to "force" a refresh, but it now
just re-runs the three calls (idempotent) and reports the same status line.

**Step 1: Update `cmd_init` to refresh, not bootstrap.**

In `src/main.rs`, replace:

```rust
async fn cmd_init(out: &Output) -> Result<(), CliError> {
    use bilitools::ipc::login;
    login::get_buvid().await?;
    login::get_bili_ticket().await?;
    login::get_uuid().await?;
    bilitools::ipc::shared::HEADERS.refresh().await?;
    out.status("initialized: buvid3, buvid4, bili_ticket, _uuid")
}
```

with:

```rust
async fn cmd_init(out: &Output) -> Result<(), CliError> {
    // Fingerprint cookies are already loaded by `context::ctx()` at startup
    // (see `AppContext::build`); this command now exists as a no-op alias
    // for backwards compatibility and to give the user a way to re-warm
    // the fingerprint after a long idle period.
    bilitools::ipc::shared::HEADERS.refresh().await?;
    let cookie = bilitools::ipc::shared::HEADERS.cookie().await;
    let summary = if cookie.contains("SESSDATA=") {
        "logged in + fingerprint present"
    } else {
        "fingerprint present (anonymous)"
    };
    out.status(summary)
}
```

**Step 2: Run the full test suite — expect 138 still passing.**

Run:
```bash
cargo test --lib
```

Expected: **138 passed**.

**Step 3: Commit.**

```bash
git add src/main.rs
git commit -m "refactor(init): make `init` a backwards-compat alias (fingerprint is now auto)"
```

---

## Task 4: End-to-end verification — `bilitools search "原神"` returns real results

**Why this is its own task:** Everything before this could be tested in
unit; this is the integration check that proves we actually fixed the user
visible bug.

**Step 1: Build the new binary.**

Run:
```bash
cargo build --release
```

Expected: `Finished release [optimized] target` with **0 errors**. Warnings
are fine.

**Step 2: Run the real search.**

Run:
```bash
./target/release/bilitools search "原神 演示" --limit 5
```

Expected: a table with at least 5 rows, each with `BV号 / 标题 / UP主 / 时长 /
播放量 / 投稿时间`. Titles should contain the words `原神` and `演示` (with
HTML `<em>` highlighting stripped). If the response says `HTTP 412` or
`code != 0`, something is still wrong — go back to Task 2 and verify the
fingerprint test is actually green in a fresh `cargo test` run.

**Step 3: Run the JSON variant.**

Run:
```bash
./target/release/bilitools search "原神" --limit 3 --json
```

Expected: a JSON array of 3 objects with fields `bvid, title, author,
duration, play, pubdate`. Verify the JSON parses with
`python3 -c "import json,sys; d=json.load(sys.stdin); print(len(d), 'items')"`.

**Step 4: Run the danmaku command too (bonus check).**

Run:
```bash
./target/release/bilitools danmaku BV1R1e4zKEh1 --format xml -o /tmp/dm-test
```

Expected: status line `saved 11 xml files to /tmp/dm-test/...` (B 站 ships
historical XML shards of a few KB each; no nav 风控 involved here). If
`comment.bilibili.com` returns 412 instead of 200, it means the IP has
been throttled — try again in 60 s, the fix is correct.

**Step 5: Commit the verification log (optional but recommended).**

If you want a permanent record of the E2E pass, append a snippet to
`docs/e2e-2026-06-11.md`:

```bash
cat >> docs/e2e-2026-06-11.md <<'EOF'

## Search with fingerprint bootstrap

- `./target/release/bilitools search "原神 演示" --limit 5` → 5 rows ✓
- `./target/release/bilitools search "原神" --limit 3 --json` → JSON[3] ✓
- `./target/release/bilitools danmaku BV1R1e4zKEh1 --format xml` → 11 shards ✓
EOF
git add docs/e2e-2026-06-11.md
git commit -m "docs(e2e): log search + danmaku E2E pass after fingerprint bootstrap fix"
```

---

## Acceptance Criteria

- [x] `cargo test --lib` shows **138 passed**.
- [x] `cargo build --release` succeeds.
- [x] `./target/release/bilitools search "原神" --limit 5` returns a table
      with real titles (no `HTTP 412`, no `code != 0`).
- [x] `--json` variant returns valid JSON parseable by `json.load`.
- [x] `bilitools danmaku BVxxx` succeeds.
- [x] `bilitools init` still works and reports the new status line.

## Out of Scope (deliberate)

- The 4K / `fnval=4048` HDR / Dolby Vision path — user dropped it on 2026-06-11.
- Persistent aria2c daemon — current per-process start/kill is fine.
- DanmakuFactory CLI install — XML→ASS upgrade is an open task but does not
  block search/danmaku retrieval.
- Persisting buvid3/buvid4/bili_ticket to SQLite — B 站 rotates these
  server-side; re-fetch on every boot is the right behavior.
