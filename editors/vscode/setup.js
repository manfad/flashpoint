// First-run setup wizard. Flow (matches jjckpt-vscode's marketplace UX):
//   1. Check the `fp` binary — offer an install terminal if missing.
//   2. Ask which AI agents you use (tick list; detected ones pre-ticked,
//      each row shows its current hook state from `fp check`).
//   3. Proceed -> `fp setup --agents ... --yes` wires hooks + PATH + excludes.
// Unlike jjckpt there is no JS engine to install and no jj/git/node
// prerequisites: the native `fp` binary owns all the wiring (config formats,
// .backup files, retiring jjckpt hooks), so the wizard is the VS Code face
// on top of the CLI — one source of truth for the hook configs.
const vscode = require("vscode");
const cp = require("child_process");
const fs = require("fs");
const os = require("os");
const path = require("path");

const SETUP_PROMPT_SEEN_KEY = "flashpoint.setupPromptSeen";
const INSTALL_CMD = "npm install -g @manfad99/flashpoint";

function home() {
  return os.homedir();
}

// Common install locations for fp, per platform. Keep in sync with
// extension.js candidateBinaries(). GUI apps often launch without the shell
// PATH; add the usual dirs so detection and execution agree.
function fpBinDirs() {
  if (process.platform === "win32") {
    const localAppData = process.env.LOCALAPPDATA || path.join(home(), "AppData", "Local");
    return [path.join(localAppData, "flashpoint", "bin"), path.join(home(), ".cargo", "bin")];
  }
  return [path.join(home(), ".local", "bin"), path.join(home(), ".cargo", "bin")];
}
function extendedEnv() {
  const env = Object.assign({}, process.env);
  env.PATH = fpBinDirs().join(path.delimiter) + path.delimiter + (env.PATH || "");
  return env;
}

function fpExec(bin, args) {
  return new Promise((resolve) => {
    cp.execFile(
      bin,
      args,
      { env: extendedEnv(), windowsHide: true, maxBuffer: 4 * 1024 * 1024 },
      (err, stdout, stderr) => {
        resolve({ ok: !err, stdout: String(stdout || ""), stderr: String(stderr || "") });
      }
    );
  });
}

async function binaryOk(bin) {
  return (await fpExec(bin, ["--version"])).ok;
}

// ---- agents ----------------------------------------------------------------
// Keep in sync with SPEEDSTERS in src/setup.rs.
const AGENTS = [
  { slug: "claude-code", label: "Claude Code", dirRel: ".claude" },
  { slug: "codex", label: "Codex", dirRel: ".codex" },
  { slug: "cursor", label: "Cursor", dirRel: ".cursor" },
  { slug: "gemini", label: "Gemini", dirRel: ".gemini" },
  { slug: "antigravity", label: "Antigravity", dirRel: ".gemini/antigravity-cli" },
  { slug: "opencode", label: "OpenCode", dirRel: ".config/opencode" },
  { slug: "vscode-copilot", label: "VS Code Copilot", dirRel: ".copilot" },
];
function detectAgents() {
  return AGENTS.filter((a) => fs.existsSync(path.join(home(), a.dirRel)));
}

// Per-agent hook state from `fp check` output lines ("  Claude Code      wired (...)"),
// so the tick list can show wired / stale / jjckpt next to each entry.
async function hookStates(bin) {
  const states = new Map();
  const { stdout } = await fpExec(bin, ["check"]); // exit 1 just means "needs attention"
  for (const line of stdout.split(/\r?\n/)) {
    const trimmed = line.trim();
    for (const a of AGENTS) {
      if (trimmed.startsWith(a.label + " ")) {
        states.set(a.slug, trimmed.slice(a.label.length).trim());
      }
    }
  }
  return states;
}

// Everything green in `fp check`? Used to decide whether to offer setup at all.
async function checkPasses(bin) {
  return (await fpExec(bin, ["check"])).ok;
}

// ---- orchestration ---------------------------------------------------------
// Open a terminal pre-filled with the install command (NOT auto-run) so the
// user reviews it and presses Enter themselves.
function openInstallTerminal() {
  const term = vscode.window.createTerminal("flashpoint install");
  term.show();
  term.sendText(INSTALL_CMD, false);
}

async function runSetup(context, resolveBin, onDone) {
  const bin = resolveBin();

  // 0) Binary check ----------------------------------------------------------
  // Without `fp` there is nothing to wire (the CLI owns the hook configs), so
  // offer the install command and stop; the user re-runs setup afterwards.
  if (!(await binaryOk(bin))) {
    const choice = await vscode.window.showWarningMessage(
      "Flashpoint: the `fp` binary was not found. Open a terminal with the npm install command ready to run?",
      "Open install terminal",
      "Cancel"
    );
    if (choice === "Open install terminal") {
      openInstallTerminal();
      vscode.window.showInformationMessage(
        "Run “Flashpoint: Setup” again once the install finishes to wire your agent hooks."
      );
    }
    return;
  }

  // 1) Which agents? ---------------------------------------------------------
  // Always show the full list so the user can wire an agent even if it isn't
  // installed on this machine yet (detected ones are pre-ticked). Picking none
  // is fine — anchors still work via the manual Anchor command.
  // ignoreFocusOut keeps the picker open even if something steals focus.
  const found = new Set(detectAgents().map((a) => a.slug));
  const states = await hookStates(bin);
  const picks = await vscode.window.showQuickPick(
    AGENTS.map((a) => ({
      label: a.label,
      description: [found.has(a.slug) ? "detected" : "", states.get(a.slug) || ""]
        .filter(Boolean)
        .join(" · "),
      picked: found.has(a.slug),
      slug: a.slug,
    })),
    {
      canPickMany: true,
      ignoreFocusOut: true,
      title: "Which AI agents do you use?",
      placeHolder:
        "Detected agents are pre-ticked. Tick any you use, then press Enter (pick none = manual anchors only).",
    }
  );
  if (picks === undefined) return; // escaped -> abort

  if (!picks.length) {
    vscode.window.showInformationMessage(
      "Flashpoint: no agents selected — use the Anchor command for manual safe points."
    );
    return;
  }

  // 2) Proceed: the CLI wires hooks + stable binary + PATH + git excludes ----
  const slugs = picks.map((p) => p.slug);
  const result = await fpExec(bin, ["setup", "--agents", slugs.join(","), "--yes"]);
  if (!result.ok) {
    vscode.window.showErrorMessage(
      "Flashpoint: setup failed — " + (result.stderr || result.stdout || "unknown error").trim()
    );
    return;
  }

  const retired = /retired jjckpt/.test(result.stdout) ? " (old jjckpt hooks retired)" : "";
  const note = (await checkPasses(bin))
    ? ""
    : " Some items still need attention — see “Flashpoint: Check Setup”.";
  vscode.window.showInformationMessage(
    `Flashpoint ready — ${slugs.length} agent hook(s) wired${retired}.` + note
  );
  if (onDone) onDone();
}

// Offer setup once on activation when something is missing or unwired.
async function maybePrompt(context, resolveBin, onDone) {
  const bin = resolveBin();
  const ok = await binaryOk(bin);
  if (ok && (await checkPasses(bin))) {
    await context.globalState.update(SETUP_PROMPT_SEEN_KEY, undefined);
    return;
  }
  if (context.globalState.get(SETUP_PROMPT_SEEN_KEY)) return;

  // Non-modal on purpose: a modal message becomes a native OS dialog (jarring,
  // steals focus); a plain notification renders as a themed VS Code toast.
  const message = ok
    ? "⚡ Flashpoint — anchors around every AI agent turn. Some agent hooks aren't wired yet; set up now?"
    : "⚡ Welcome to Flashpoint — anchors around every AI agent turn. Set up now? (Also helps install the `fp` binary.)";
  const choice = await vscode.window.showInformationMessage(message, "Set up", "Later");
  await context.globalState.update(SETUP_PROMPT_SEEN_KEY, true);
  if (choice === "Set up") await runSetup(context, resolveBin, onDone);
}

// Surface `fp check` as a themed toast (with the full report on demand).
async function showCheck(resolveBin) {
  const bin = resolveBin();
  if (!(await binaryOk(bin))) {
    vscode.window.showWarningMessage("Flashpoint: the `fp` binary was not found — run “Flashpoint: Setup”.");
    return;
  }
  const result = await fpExec(bin, ["check"]);
  const summary = result.ok
    ? "Flashpoint: all good — binary, PATH, excludes, and hooks are set up."
    : "Flashpoint: `fp check` found items needing attention.";
  const choice = await vscode.window.showInformationMessage(summary, "Show report", "Run setup");
  if (choice === "Show report") {
    const doc = await vscode.workspace.openTextDocument({ content: result.stdout, language: "plaintext" });
    await vscode.window.showTextDocument(doc, { preview: true });
  }
  return choice === "Run setup";
}

module.exports = { runSetup, maybePrompt, showCheck };
