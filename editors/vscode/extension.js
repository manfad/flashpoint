// Flashpoint VS Code extension — Agent Session view.
// Forked from jjckpt-vscode; timeline tab intentionally omitted.
// Shells out to the native `fp` binary (no jj, no Node engine required).

const vscode = require("vscode");
const { execFile } = require("child_process");
const path = require("path");
const os = require("os");
const fs = require("fs");

const SCHEME = "flashpoint";

// ---------------------------------------------------------------- binary

function candidateBinaries() {
  const configured = vscode.workspace.getConfiguration("flashpoint").get("binary");
  const list = [];
  if (configured) list.push(configured);
  list.push("fp");
  list.push(path.join(os.homedir(), ".cargo", "bin", "fp"));
  list.push(path.join(os.homedir(), ".local", "bin", "fp"));
  return list;
}

let resolvedBin = null;
function resolveBin() {
  if (resolvedBin) return resolvedBin;
  for (const cand of candidateBinaries()) {
    if (cand.includes(path.sep)) {
      if (fs.existsSync(cand)) { resolvedBin = cand; return cand; }
    } else {
      resolvedBin = cand; // bare name: let PATH resolution decide at spawn time
      return cand;
    }
  }
  resolvedBin = "fp";
  return resolvedBin;
}

function repoCwd() {
  const folders = vscode.workspace.workspaceFolders;
  return folders && folders.length ? folders[0].uri.fsPath : process.cwd();
}

function run(args, options = {}) {
  return new Promise((resolve, reject) => {
    execFile(resolveBin(), args,
      { cwd: repoCwd(), maxBuffer: 32 * 1024 * 1024, windowsHide: true },
      (err, stdout, stderr) => {
        if (err && options.rejectOnError) {
          reject(new Error(String(stderr || err.message || "command failed").trim()));
          return;
        }
        resolve(stdout != null ? String(stdout) : "");
      });
  });
}

// ---------------------------------------------------------------- naming

const AGENT_ICONS = [
  ["claude", "claude"],
  ["codex", "codex"],
  ["cursor", "cursor"],
  ["gemini", "gemini"],
  ["opencode", "opencode"],
  ["antigravity", "antigravity"],
  ["copilot", "githubcopilot"],
  ["human", "human"],
  ["vscode", "human"],
];

function agentIconName(slug) {
  const lower = String(slug || "").toLowerCase();
  for (const [needle, icon] of AGENT_ICONS) if (lower.includes(needle)) return icon;
  return "robot";
}

function prettyName(slug) {
  if (!slug) return "";
  if (slug === "human") return "You";
  return slug
    .split(/[-_]/)
    .map((w) => (w ? w[0].toUpperCase() + w.slice(1) : w))
    .join(" ");
}

// A title that is just a timestamp means "no summary available".
function isTimestampTitle(s) {
  return /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}(:\d{2})?$/.test(String(s || "").trim());
}

function checkpointLabel(ckpt, number) {
  const kind = ["human", "vscode", "checkpoint"].includes(ckpt.agent) ? "Safepoint" : "Checkpoint";
  if (ckpt.subject && !isTimestampTitle(ckpt.subject)) {
    return { label: ckpt.subject, description: `${kind} #${number} · ${prettyName(ckpt.agent)}` };
  }
  return { label: `${kind} ${number} — ${prettyName(ckpt.agent)}`, description: ckpt.time };
}

// ---------------------------------------------------------------- URIs

function ckptUri(hash) {
  return vscode.Uri.from({ scheme: SCHEME, authority: "ckpt", path: "/" + hash });
}

function revUri(filePath, rev) {
  return vscode.Uri.from({ scheme: SCHEME, path: "/" + filePath, query: rev });
}

function absoluteFileUri(relPath) {
  return vscode.Uri.file(path.join(repoCwd(), relPath));
}

// ---------------------------------------------------------------- tree

class CheckpointProvider {
  constructor(context) {
    this.context = context;
    this._emitter = new vscode.EventEmitter();
    this.onDidChangeTreeData = this._emitter.event;
    this._decoEmitter = new vscode.EventEmitter();
    this.onDidChangeFileDecorations = this._decoEmitter.event;
    this.compare = null; // { before, after, path }
    this.prereqOk = true;
    this._ckptItems = new Map();
    this._expandedCkpt = null;
  }

  refresh() {
    this._emitter.fire();
    this._decoEmitter.fire();
  }

  setPrereq(ok) {
    this.prereqOk = ok;
    this.refresh();
  }

  setCompare(compare) {
    this.compare = compare;
    this._decoEmitter.fire();
  }

  getTreeItem(item) {
    return item;
  }

  async getChildren(item) {
    if (!this.prereqOk) return [];
    if (!item) return this._roots();
    if (item.kind === "session") return item.children || [];
    if (item.kind === "checkpoint") return this._files(item.hash);
    return [];
  }

  async _roots() {
    this._ckptItems.clear();
    let stages, active;
    try {
      const [stagesRaw, activeRaw] = await Promise.all([run(["_stages"]), run(["_active"])]);
      stages = JSON.parse(stagesRaw);
      active = new Set(activeRaw.split(/\r?\n/).filter(Boolean));
    } catch {
      return [this._info("flashpoint store not readable — run `fp anchor` once")];
    }
    const current = (stages.stages || []).find((s) => s.current);
    const ckpts = current ? current.ckpts : [];
    if (!ckpts.length) return [this._info("(no anchors since last commit)")];
    return this._buildSessions(ckpts, active);
  }

  _info(text) {
    const it = new vscode.TreeItem(text, vscode.TreeItemCollapsibleState.None);
    it.kind = "info";
    return it;
  }

  _buildSessions(ckpts, active) {
    const order = [];
    const groups = new Map();
    for (const c of ckpts) {
      const key = c.session || "(none)";
      if (!groups.has(key)) { groups.set(key, []); order.push(key); }
      groups.get(key).push(c);
    }
    const agentCounts = new Map();
    return order.map((key) => {
      const group = groups.get(key);
      const agent = group[0].agent || "";
      const n = (agentCounts.get(agent) || 0) + 1;
      agentCounts.set(agent, n);
      const label = n > 1 ? `${prettyName(agent)} #${n}` : prettyName(agent) || "Session";
      const item = new vscode.TreeItem(label, vscode.TreeItemCollapsibleState.Expanded);
      item.kind = "session";
      item.contextValue = "session";
      item.description = `${group.length} anchor${group.length === 1 ? "" : "s"}`;
      item.iconPath = this._agentIcon(agent);
      item.children = this._buildCheckpoints(group, active);
      return item;
    });
  }

  _agentIcon(agent) {
    const name = agentIconName(agent);
    const media = (f) => vscode.Uri.file(path.join(this.context.extensionPath, "media", f));
    return {
      light: media(path.join("agents", `${name}-light.svg`)),
      dark: media(path.join("agents", `${name}.svg`)),
    };
  }

  _buildCheckpoints(group, active) {
    // Per-agent numbering, oldest = #1 within the visible stage.
    const byAgent = new Map();
    const numbered = [...group].reverse().map((c) => {
      const n = (byAgent.get(c.agent) || 0) + 1;
      byAgent.set(c.agent, n);
      return [c, n];
    }).reverse();

    return numbered.map(([c, number]) => {
      const onPath = active.has(c.id);
      const { label, description } = checkpointLabel(c, number);
      const item = new vscode.TreeItem(label, vscode.TreeItemCollapsibleState.Collapsed);
      item.kind = "checkpoint";
      item.hash = c.id;
      item.description = description;
      item.tooltip = `${c.id} · ${c.time}${onPath ? "" : " · off current path"}`;
      item.contextValue = onPath ? "checkpoint" : "checkpoint inactive";
      item.resourceUri = ckptUri(c.id);
      item.iconPath = this._ckptIcon(c.id, onPath);
      item.id = `ckpt:${c.id}`;
      this._ckptItems.set(c.id, item);
      return item;
    });
  }

  _ckptIcon(hash, onPath) {
    if (this.compare) {
      if (this.compare.after === hash) return new vscode.ThemeIcon("pass", new vscode.ThemeColor("charts.green"));
      if (this.compare.before === hash) return new vscode.ThemeIcon("error", new vscode.ThemeColor("charts.red"));
    }
    if (!onPath) return new vscode.ThemeIcon("circle-outline", new vscode.ThemeColor("disabledForeground"));
    const media = (f) => vscode.Uri.file(path.join(this.context.extensionPath, "media", f));
    return { light: media("pin-light.svg"), dark: media("pin.svg") };
  }

  async _files(hash) {
    let raw = "";
    try { raw = await run(["_files", hash]); } catch { return []; }
    const rows = raw.split(/\r?\n/).filter(Boolean).map((line) => {
      const [status, ...rest] = line.split("\t");
      return { status: (status || "").trim(), path: rest.join("\t") };
    });
    return rows.map((row) => {
      const item = new vscode.TreeItem(path.posix.basename(row.path), vscode.TreeItemCollapsibleState.None);
      item.kind = "file";
      item.hash = hash;
      item.path = row.path;
      item.status = row.status;
      item.description = path.posix.dirname(row.path) === "." ? "" : path.posix.dirname(row.path);
      item.resourceUri = absoluteFileUri(row.path).with({ fragment: `${hash}:${row.status}` });
      item.command = { command: "flashpoint.openDiff", title: "Open Diff", arguments: [item] };
      return item;
    });
  }

  // FileDecorationProvider ------------------------------------------------
  provideFileDecoration(uri) {
    if (uri.scheme === SCHEME && uri.authority === "ckpt") {
      const hash = uri.path.replace(/^\//, "");
      if (this.compare) {
        if (this.compare.after === hash) return { color: new vscode.ThemeColor("charts.green") };
        if (this.compare.before === hash) return { color: new vscode.ThemeColor("charts.red") };
      }
      return undefined;
    }
    if (uri.scheme === "file" && uri.fragment) {
      const [, status] = uri.fragment.split(":");
      if (!status) return undefined;
      const colors = { A: "charts.green", M: "charts.yellow", D: "charts.red", R: "charts.blue" };
      return {
        badge: status,
        color: new vscode.ThemeColor(colors[status] || "foreground"),
      };
    }
    return undefined;
  }
}

// ---------------------------------------------------------------- diff

async function openDiff(item, provider) {
  if (!item || !item.hash || !item.path) return;
  if (item.status === "A") {
    const left = revUri(item.path, item.hash + "-");
    const right = revUri(item.path, item.hash);
    await vscode.commands.executeCommand("vscode.diff", left, right,
      `${path.posix.basename(item.path)} (New)`, { preview: true });
    if (provider) provider.setCompare({ before: null, after: item.hash, path: item.path });
    return;
  }
  let before = "";
  try { before = (await run(["_parent", item.hash])).trim(); } catch { /* root */ }
  const left = revUri(item.path, item.hash + "-");
  const right = revUri(item.path, item.hash);
  const title = `${path.posix.basename(item.path)} (${before ? before.slice(0, 8) : "root"}) ↔ (${item.hash.slice(0, 8)})`;
  await vscode.commands.executeCommand("vscode.diff", left, right, title, { preview: true });
  if (provider) provider.setCompare({ before: before || null, after: item.hash, path: item.path });
}

async function compareCurrent(item) {
  if (!item || !item.hash) return;
  let filePath = item.path;
  if (!filePath) {
    const raw = await run(["_files", item.hash]);
    const files = raw.split(/\r?\n/).filter(Boolean).map((l) => l.split("\t").slice(1).join("\t"));
    if (!files.length) {
      vscode.window.showInformationMessage("Anchor has no changed files.");
      return;
    }
    filePath = files.length === 1 ? files[0] : await vscode.window.showQuickPick(files, { placeHolder: "Compare which file with current?" });
    if (!filePath) return;
  }
  await vscode.commands.executeCommand(
    "vscode.diff",
    revUri(filePath, item.hash),
    absoluteFileUri(filePath),
    `${filePath} · anchor vs current`,
    { preview: true }
  );
}

// ---------------------------------------------------------------- actions

async function runUserCommand(args, successMsg, provider) {
  try {
    await run(args, { rejectOnError: true });
    if (successMsg) vscode.window.setStatusBarMessage(successMsg, 4000);
    if (provider) provider.refresh();
  } catch (err) {
    vscode.window.showErrorMessage(`flashpoint: ${err.message}`);
  }
}

async function timetravelWithPreview(item, provider) {
  if (!item || !item.hash) return;
  const choice = await vscode.window.showWarningMessage(
    `Timetravel to this anchor? Your files are rewritten to that safe point. ` +
    `Un-anchored changes are sealed automatically first, and nothing forks unless you change something afterwards.`,
    { modal: true },
    "Preview Changed Files",
    "Timetravel"
  );
  if (choice === "Preview Changed Files") {
    await compareCurrent(item);
    return;
  }
  if (choice === "Timetravel") {
    await runUserCommand(["timetravel", item.hash, "--yes"], "Timetraveled.", provider);
  }
}

async function showStatusDoc() {
  const out = await run(["status"]);
  const doc = await vscode.workspace.openTextDocument({ content: out || "(no output)", language: "plaintext" });
  await vscode.window.showTextDocument(doc, { preview: true });
}

// ---------------------------------------------------------------- prereqs

async function checkBinary() {
  try {
    await run(["--version"], { rejectOnError: true });
    return true;
  } catch {
    return false;
  }
}

// ---------------------------------------------------------------- activate

function activate(context) {
  const provider = new CheckpointProvider(context);

  const tree = vscode.window.createTreeView("flashpoint.tree", {
    treeDataProvider: provider,
    showCollapseAll: false,
  });
  context.subscriptions.push(tree);
  context.subscriptions.push(vscode.window.registerFileDecorationProvider(provider));

  // Accordion: one open file list at a time; expanding selects the diff pair.
  context.subscriptions.push(tree.onDidExpandElement(async (ev) => {
    const item = ev.element;
    if (item.kind !== "checkpoint") return;
    provider._expandedCkpt = item.hash;
    let before = "";
    try { before = (await run(["_parent", item.hash])).trim(); } catch { /* root */ }
    provider.setCompare({ before: before || null, after: item.hash, path: null });
  }));

  const contentProvider = {
    provideTextDocumentContent(uri) {
      if (uri.authority === "ckpt") return "";
      const rev = uri.query;
      const file = uri.path.replace(/^\//, "");
      if (!rev || !file) return "";
      return run(["_show", rev, file]);
    },
  };
  context.subscriptions.push(
    vscode.workspace.registerTextDocumentContentProvider(SCHEME, contentProvider)
  );

  const cmd = (id, fn) => context.subscriptions.push(vscode.commands.registerCommand(id, fn));
  cmd("flashpoint.refresh", () => provider.refresh());
  cmd("flashpoint.anchor", () =>
    runUserCommand(["anchor", "--speedster", "human", "--session", "human"], "Anchored.", provider));
  cmd("flashpoint.timetravel", (item) => timetravelWithPreview(item, provider));
  cmd("flashpoint.openDiff", (item) => openDiff(item, provider));
  cmd("flashpoint.compareCurrent", (item) => compareCurrent(item));
  cmd("flashpoint.status", () => showStatusDoc());
  cmd("flashpoint.recheck", () => refreshPrereqs());

  // Status bar: quick anchor.
  const statusItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
  statusItem.text = "$(shield) Anchor";
  statusItem.tooltip = "Flashpoint: seal the current working copy as a safe point";
  statusItem.command = "flashpoint.anchor";
  statusItem.show();
  context.subscriptions.push(statusItem);

  // Poll the op-log head while the view is visible.
  let lastTip = "";
  let pollTimer = null;
  let pollInFlight = false;
  async function poll() {
    if (pollInFlight || !tree.visible) return;
    pollInFlight = true;
    try {
      const tip = (await run(["_tip"])).trim();
      if (tip && tip !== lastTip) {
        lastTip = tip;
        provider.refresh();
      }
    } finally {
      pollInFlight = false;
    }
  }
  function syncPolling() {
    const seconds = Math.max(1, vscode.workspace.getConfiguration("flashpoint").get("pollSeconds") || 2);
    if (pollTimer) clearInterval(pollTimer);
    pollTimer = setInterval(poll, seconds * 1000);
    context.subscriptions.push({ dispose: () => clearInterval(pollTimer) });
  }
  syncPolling();
  context.subscriptions.push(tree.onDidChangeVisibility(() => poll()));
  context.subscriptions.push(vscode.window.onDidChangeWindowState((st) => { if (st.focused) poll(); }));
  context.subscriptions.push(vscode.workspace.onDidChangeConfiguration((ev) => {
    if (ev.affectsConfiguration("flashpoint.pollSeconds")) syncPolling();
    if (ev.affectsConfiguration("flashpoint.binary")) { resolvedBin = null; refreshPrereqs(); }
  }));

  async function refreshPrereqs() {
    const ok = await checkBinary();
    vscode.commands.executeCommand("setContext", "flashpoint.fpMissing", !ok);
    provider.setPrereq(ok);
  }
  refreshPrereqs();

  // Keep the store out of the explorer.
  const files = vscode.workspace.getConfiguration("files");
  const exclude = { ...(files.get("exclude") || {}) };
  if (!exclude["**/.jj"] || !exclude["**/.fp"]) {
    exclude["**/.jj"] = true;
    exclude["**/.fp"] = true;
    files.update("exclude", exclude, vscode.ConfigurationTarget.Global).then(undefined, () => {});
  }
}

function deactivate() {}

module.exports = { activate, deactivate };
