# flashpoint

**README.md is the single source of truth for this project.** Read it fully
before doing anything. It contains the product model, the settled decisions
(marked "do not relitigate"), vocabulary, and the roadmap. Decisions there were
hard-won across long discussions — do not reopen them; if the user's idea
evolves, update README.md in the same change so it stays authoritative.

Quick orientation:

- Checkpoint tool for AI-agent coding sessions. Rust binary depending on the
  `jj-lib` crate (never fork jj, never shell out to the jj CLI).
- Three commands: `anchor`, `timetravel <id>`, `diff <id>`. Flashpoint is the
  fork EVENT (fork materializes lazily on first change after a timetravel),
  not a command. No `undo`.
- Agents are "speedsters". No timeline is ever lost.
- Predecessor with all the proven UX and hook configs: `~/jjckpt-vscode`
  (Node engine `bin/jjckpt.js`, VS Code extension). Its `_timeline`
  JSON porcelain is the compatibility contract for UI surfaces.
- Current roadmap position: steps 1–5 done (working `fp` binary: anchor, log,
  diff, timetravel, hook, `_timeline`). Next: step 6, port the VS Code
  extension.
