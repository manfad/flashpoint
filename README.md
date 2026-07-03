# flashpoint

A checkpoint tool for AI-agent coding sessions. One native binary, three
commands, nothing to configure. Successor to
[jjckpt](https://github.com/manfad/jjckpt-vscode) (`~/jjckpt-vscode`), whose
Node engine + `jj` CLI dependency this replaces.

## The pitch

AI agents rewrite your files every turn. flashpoint saves an **anchor** (safe
point) around every turn so you can inspect what changed or **timetravel**
back — without touching your real git history. Change something after
timetraveling and reality forks onto a new timeline (a **flashpoint** — the
tool is named after its signature event). Install one binary. That's the
whole setup.

## Vocabulary (The Flash universe — this is the brand)

- **speedster** — an agent (Claude Code, Codex, Cursor, ...). Every speedster's
  turns are grouped as a session; the human's between-turn edits are their own
  session.
- **anchor** — a sealed safe point of the working copy.
- **timeline** — a path of anchors. Timeline #1 is where you start; flashpoints
  create more. **No timeline is ever lost** — every timeline, including
  abandoned futures, stays timetravel-able forever.
- **flashpoint** — the fork event (see below), and the first anchor of a new
  timeline.

## Commands

| command | what it does |
|---|---|
| `anchor` | seal the current working copy as a safe point |
| `timetravel <id>` | put files back exactly as they were at an anchor — nothing forks yet |
| `diff <id>` | what changed at / since an anchor |

Three commands. **Flashpoint is not a command — it's an event.** Timetravel
alone never forks anything: go back, look around, timetravel forward again,
and the timeline is untouched. But the moment you *change something* after
timetraveling, reality forks: the next anchor sealed becomes the **flashpoint
node** — first anchor of a new timeline. Later anchors on the old timeline
survive, marked *off current path*, and you can always timetravel back onto
timeline #1 (or any timeline). Like the movie: going back doesn't break the
timeline; changing something does.

Because forks only materialize on first change, an accidental timetravel
leaves no mess — which is why there is no `undo` command.

### Timetravel safety rules

- If the workspace has un-anchored changes when the user timetravels, seal a
  **safety anchor automatically first** (and say so). Timetravel must never be
  able to destroy work — not even work that was never anchored.
- Confirm with a changed-file count before rewriting the workspace (as jjckpt
  does today).
- Safe-zone exclusions (`.env`, local config globs) are never rewritten.

### Diff modes

`diff` must support three comparisons: anchor vs its parent (what that turn
changed), anchor vs current workspace (what restoring would rewrite — shown
before timetravel), and anchor vs anchor.

## When anchors are created

1. **Speedster stop-hook** — every agent turn ends → anchor.
2. **Speedster pre-turn hook** — anchor only if the working copy differs from
   the latest anchor (seals the human's between-turn edits as a Human
   safepoint; no diff → no anchor).
3. **Manually** — `flashpoint anchor`.

## Anchor metadata format (settled)

Clean break from jjckpt — fp reads/writes ONLY its own format (existing
jjckpt checkpoints do not appear in fp output). An anchor is a jj commit whose
description is the anchor title (first line; falls back to a timestamp) plus
trailers:

```
<title>

Fp-Session: <session id>
Fp-Speedster: <agent slug, e.g. claude-code, human>
Fp-Phase: pre | post | manual
Fp-Base: <git HEAD sha, omitted when not a git repo>
```

Anchor discovery keys off descriptions containing `Fp-Session:`. User-facing
anchor ids are jj change-id short prefixes (8 chars, reverse-hex alphabet).

## Store location (settled)

Always `<project>/.jj` (jj-lib hardcodes the dir name), auto-added to
`.git/info/exclude` when the project is a real git repo. No external
XDG-keyed store (that jjckpt mechanism is dropped).

## Git commit awareness (settled)

Every anchor records the real repo's git `HEAD` commit id at seal time as
metadata (a cheap read via gitoxide — no git hooks, the real repo is still
never written to). Commits are **grouping boundaries, never fork triggers**:
UIs and the `_timeline` porcelain group anchors under the commit they were
sealed on, like session grouping. A new commit does NOT start a new timeline —
timelines are derived from the anchor parent graph, and only a flashpoint
(changing the past after a timetravel) forks one. Git commits are also noisy
signals (rebase, amend, branch switch all move `HEAD` without meaning "new era
of work"), so they'd produce spurious splits if they forked anything.

## The core decision (settled — do not relitigate)

flashpoint is a **new small Rust binary that depends on the `jj-lib` crate**.

- **NOT a fork of jj.** A stripped fork keeps the hardest 20% of jj's code
  (snapshotting, storage, op store) while losing upstream maintenance. Worst
  option; rejected.
- **NOT shelling out to the `jj` CLI.** That's what jjckpt does today; it works
  but requires users to install jj + Node, and parses unstable CLI output.
- **NOT git plumbing.** Git can do snapshot/restore/diff cheaply but has no
  operation log — undo would be hand-built. jj-lib's Transaction model gives
  undo for free.

jj-lib is *designed* for this: the jj project is split into `jj-lib` (all VCS
machinery, no terminal I/O) and `jj-cli` (a thin frontend). Docs explicitly
bless third-party frontends: https://docs.jj-vcs.dev/latest/technical/architecture/
flashpoint is just another frontend, a peer of `jj-cli`.

## Concept → jj-lib component map

| concept | jj-lib pieces |
|---|---|
| anchor | `WorkingCopy`/`TreeState` snapshot + `Transaction` |
| timetravel | `Store` (load tree) + `WorkingCopy` checkout, working copy re-parented onto the target anchor |
| flashpoint (event) | nothing extra — the next anchor sealed after a timetravel is a commit parented on the target; the fork exists the moment it lands. Diff-gated sealing means no change → no fork |
| diff | tree comparison via `Backend`/`Store` |

Backend: stay on **GitBackend** (gitoxide). Objects are git-format on disk, so
"export a good anchor to a real git branch" stays a cheap feature. The store
lives in its own dir (as jjckpt does with `.jj/` + local git exclude), so the
user's real git repo is never touched.

## Known risks

## Concurrency (settled)

jj-lib locks around every repo mutation, so simultaneous hooks can never
corrupt the store. The remaining cases and their rulings:

1. **Human edits between agent turns** — handled by design: the diff-gated
   pre-turn hook seals them as a Human safepoint.
2. **Human edits during an agent turn** — accepted: the stop-hook anchor
   co-mingles them with the agent's work. Anchor attribution means "who
   triggered the seal," not "who authored every line." The diff is still
   true, just shared.
3. **Two speedsters in the same directory — ANTI-PATTERN, unsupported.**
   This workflow is broken below flashpoint (the agents' file writes race
   regardless of any checkpoint tool). It can NOT produce parallel timelines:
   one directory has one file state, so their anchors just interleave,
   co-mingled, on a single timeline. Graceful degradation only (locking
   guarantees no corruption). The recommended pattern: one worktree per
   speedster — parallel timelines require parallel working copies, so each
   speedster gets its own universe with its own store. **v1 does detect it**:
   the pre-turn hook sees another speedster's turn-active marker and
   warns/blocks BEFORE the second agent writes anything — "another speedster
   is mid-turn here; run this agent in its own worktree, or wait." Detection
   is cheap; what v1 does NOT do is auto-fork, because a hook cannot redirect
   an already-running agent into another folder — the graph would show two
   timelines while both agents still write to one. (Future "multiverse mode":
   flashpoint launches each speedster into its own hidden jj workspace, one
   store, true parallel timelines, merge on demand. That requires flashpoint
   to become an agent launcher — flashpoint 2.0, out of scope.)
4. **Timetravel while a speedster is mid-turn** — the one dangerous case:
   restore and agent writes interleave into a garbage workspace (recoverable
   via the pre-turn anchor, but still). Guarded: the pre-turn hook leaves a
   turn-active marker (timestamped, auto-expires ~30 min so a crashed agent
   can't wedge it); `timetravel` sees it and warns "speedster mid-turn —
   continue anyway?".

## Known risks

`jj-lib` has **no API stability guarantees** (0.43.0 as of 2026-07-03,
versioned in lockstep with jj's ~monthly releases; breaking changes routine).
Mitigation: pin the version, upgrade deliberately every few months, let the
compiler flag breakages. Still a better contract than parsing pre-1.0 CLI
output at runtime.

## What carries over from jjckpt (`~/jjckpt-vscode`)

The surfaces are the bulk of that repo and they survive unchanged — they just
call `flashpoint` instead of `jjckpt`:

- speedster hook configs (pre-turn Human safepoint + post-turn checkpoint) for
  Claude Code, Codex, Cursor, Gemini, Antigravity, OpenCode, VS Code Copilot
- the VS Code extension (sidebar: Sessions ▸ Safe Points ▸ Files, native diff)
  — **first-priority surface**
- the shared timeline view-model / `_timeline` porcelain contract — port its
  JSON output shape verbatim; every UI already renders it
- LLM checkpoint titles (pre-turn prompt stash + detached titler)
- session grouping, "off current path" semantics, safe-zone exclusions

flashpoint removes the prerequisites (jj CLI, system Node for hooks), not the
product.

Naming migration from jjckpt: jjckpt's go-back command was called "flashpoint"
(with `restore` as a legacy alias). In this product that action is
**timetravel**; "flashpoint" now means only the fork event / the tool itself.
When porting UI surfaces, rename the go-back button/command accordingly.

## Roadmap

1. ~~**Spike.**~~ DONE — jj-lib 0.43.0 (pinned exact), API proved tolerable in
   under an hour. Stores are readable by the stock jj CLI.
2. ~~`anchor` + `log`.~~ DONE (diff-gated sealing; `jj commit`-style seal +
   fresh empty wc commit).
3. ~~`diff` + `timetravel`.~~ DONE (three diff modes; changed-file-count
   confirm with `--yes` escape; auto safety anchor; safe-zone exclusions;
   turn-active guard).
4. ~~flashpoint event.~~ DONE — falls out of jj-lib: timetravel checks out an
   empty wc commit on the target, which jj-lib auto-abandons if left
   unchanged (zero-trace accidental timetravel) and which the next anchor
   seals into the flashpoint node.
5. ~~`_timeline` porcelain~~ DONE (jjckpt row/laneCount shape, Fp-* trailers)
   plus `fp hook --speedster <slug> --phase pre|post` (stdin payload JSON,
   Human safepoints, prompt-derived titles, turn-active marker,
   second-speedster block).
6. ~~VS Code extension.~~ DONE — forked from jjckpt into `editors/vscode/`
   (Agent Session tree only; the timeline tab was dropped by decision).
   Ships as `flashpoint-*.vsix`; talks to the native `fp` binary via the
   ported porcelain (`_stages`, `_active`, `_files`, `_parent`, `_show`,
   `_tip`), so it needs no jj CLI and no Node engine. jjckpt's undo/fork
   buttons were removed (no `undo` by design; forks are the lazy flashpoint
   event).
7. Release binaries (mac/linux/win) — this is the new cost fork/CLI didn't have.

Dev note: never run the bare `jj` CLI inside an fp-managed directory for
inspection — it snapshots the working copy under jj's own rules (no safe-zone)
and pollutes the store. Always `jj --ignore-working-copy`.

## Why no `undo`

The lazy-fork model makes it unnecessary: an accidental timetravel with no
changes leaves zero trace (no anchor sealed → no fork), and content mistakes
are always recoverable by timetraveling again — no anchor is ever destroyed.
jj-lib records an operation log on every transaction regardless, so an `undo`
command could be added later for free if real usage ever demands one.

## Non-goals

- Reimplementing any VCS machinery ourselves.
- Exposing jj concepts (revsets, bookmarks, rebase) to users. jj is an
  implementation detail; the compat escape hatch is git export, not jj access.
- A second backend. One engine, made invisible.
- A hand-rolled store (`anchors.json` + snapshot blobs + refs) or a
  stored-per-anchor `timelineId`. The store IS a shadow git-format repo via
  jj-lib's GitBackend; timelines are DERIVED from the parent graph (a fork is
  just a second child), so they can never go stale. Reviews that suggest
  "build a shadow git repo yourself" are describing what jj-lib already is.

## UI defaults

Timeline branches are visible, but the default view follows ONE active
timeline (linear, like jjckpt's default); off-path timelines are muted. The
full DAG stays behind a power-mode toggle so users don't get lost.
