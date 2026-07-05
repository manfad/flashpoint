// Flashpoint VS Code extension — Agent Session view.
// Forked from jjckpt-vscode; timeline tab intentionally omitted.
// Shells out to the native `fp` binary (no jj, no Node engine required).

const vscode = require("vscode");
const setup = require("./setup");
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

  // Needed by TreeView.reveal (timeline click → expand anchor in this tree).
  getParent(item) {
    if (item.kind === "checkpoint") return item.parentSession || null;
    if (item.kind === "file") return this._ckptItems.get(item.hash) || null;
    return null;
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
      item.id = `session:${key}`;
      item.contextValue = "session";
      item.description = `${group.length} anchor${group.length === 1 ? "" : "s"}`;
      item.iconPath = this._agentIcon(agent);
      item.children = this._buildCheckpoints(group, active);
      for (const child of item.children) child.parentSession = item;
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
    const strike = (s) => String(s).replace(/[^\s]/g, "$&\u0336");
    return rows.map((row) => {
      const deleted = row.status === "D";
      const basename = path.posix.basename(row.path);
      const dirname = path.posix.dirname(row.path);
      const item = new vscode.TreeItem(deleted ? strike(basename) : basename, vscode.TreeItemCollapsibleState.None);
      item.kind = "file";
      item.hash = hash;
      item.path = row.path;
      item.status = row.status;
      item.description = dirname === "." ? "" : deleted ? strike(dirname) : dirname;
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
      const relPath = path.relative(repoCwd(), uri.fsPath).split(path.sep).join("/");
      if (this.compare && this.compare.path === relPath) {
        if (this.compare.after && uri.fragment.startsWith(`${this.compare.after}:`)) {
          return { badge: status, color: new vscode.ThemeColor("charts.green") };
        }
        if (this.compare.before && uri.fragment.startsWith(`${this.compare.before}:`)) {
          return { badge: status, color: new vscode.ThemeColor("charts.red") };
        }
      }
      return { badge: status };
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

// Multi-file diff of an anchor vs its parent (used by timeline node clicks).
async function openAnchorChanges(hash, provider) {
  let raw = "";
  try { raw = await run(["_files", hash]); } catch { return; }
  let before = "";
  try { before = (await run(["_parent", hash])).trim(); } catch { /* root */ }
  if (provider) provider.setCompare({ before: before || null, after: hash, path: null });
  const files = raw.split(/\r?\n/).filter(Boolean).map((line) => {
    const [status, ...rest] = line.split("\t");
    return { status: (status || "").trim(), path: rest.join("\t") };
  });
  if (!files.length) {
    vscode.window.setStatusBarMessage("flashpoint: anchor changed no files", 3000);
    return before || null;
  }
  const triples = files.map((f) => [
    absoluteFileUri(f.path),
    revUri(f.path, hash + "-"),
    revUri(f.path, hash),
  ]);
  try {
    await vscode.commands.executeCommand(
      "vscode.changes",
      `Anchor ${hash.slice(0, 8)} vs ${before ? before.slice(0, 8) : "root"}`,
      triples
    );
  } catch {
    await openDiff({ hash, path: files[0].path, status: files[0].status }, provider);
  }
  return before || null;
}

// ---------------------------------------------------------------- timeline

class TimelineWebviewProvider {
  constructor(onSelect) {
    this.onSelect = onSelect;
    this.view = null;
  }

  resolveWebviewView(view) {
    this.view = view;
    view.webview.options = { enableScripts: true };
    view.webview.html = timelineHtml();
    view.webview.onDidReceiveMessage((msg) => {
      if (msg && msg.type === "select" && msg.id) this.onSelect(msg.id);
      if (msg && msg.type === "ready") this.update();
    });
    view.onDidChangeVisibility(() => { if (view.visible) this.update(); });
    this.update();
  }

  get visible() {
    return !!(this.view && this.view.visible);
  }

  async update() {
    if (!this.view) return;
    try {
      const raw = await run(["_timeline"], { rejectOnError: true });
      this.view.webview.postMessage({ type: "model", model: JSON.parse(raw) });
    } catch (err) {
      this.view.webview.postMessage({ type: "error", message: String(err.message || err) });
    }
  }

  makePrime(id) {
    if (this.view && id) this.view.webview.postMessage({ type: "prime", id });
  }
}

function timelineHtml() {
  const nonce = Math.random().toString(36).slice(2);
  return `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<meta http-equiv="Content-Security-Policy"
      content="default-src 'none'; style-src 'unsafe-inline'; script-src 'nonce-${nonce}';">
<style>
  html, body { margin: 0; padding: 0; width: 100%; background: transparent; }
  body { overflow: auto; }
  /* Anchor the graph to the bottom so it grows upward like a tree. */
  #wrap {
    position: relative;
    width: 100%;
    min-height: 100vh;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    align-items: stretch;
  }
  canvas { display: block; width: 100%; cursor: default; }
  #tip {
    position: fixed; display: none; z-index: 10; pointer-events: none;
    background: var(--vscode-editorHoverWidget-background, #252526);
    color: var(--vscode-editorHoverWidget-foreground, #ccc);
    border: 1px solid var(--vscode-editorHoverWidget-border, #454545);
    border-radius: 6px; padding: 5px 10px;
    font: 12px var(--vscode-font-family, sans-serif);
    max-width: 260px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.35);
  }
  #tip .time { opacity: 0.7; font-size: 11px; margin-top: 1px; font-variant-numeric: tabular-nums; }
  #msg { padding: 10px; font: 12px var(--vscode-font-family, sans-serif); opacity: 0.8; }
</style>
</head>
<body>
<div id="wrap"><canvas id="c"></canvas><div id="msg"></div></div>
<div id="tip"></div>
<script nonce="${nonce}">
const vscode = acquireVsCodeApi();
const PALETTE = ["#3794ff", "#e5399e", "#8bc34a", "#f5a623", "#b180d7", "#26c6da", "#ef5350"];
const ROW_H = 30, LANE_W = 18, PAD = 14, R = 5.5;

let nodes = [];   // {id,label,time,current,lane,x,y,color,onCurrentPath,parents[]}
let modelRows = [];
let selected = null;   // {after, before} — highlighted pair after a click
let hoverId = null;
let primeId = null;
const SEL_GREEN = "#7ee287";
const SEL_RED = "#f14c4c";
const SEL_EDGE = "#ffffff";
const CURRENT_EDGE = "#3794ff";
const ALT_EDGE = "#7a7a7a";

function layout(model) {
  const rows = model.rows || [];
  const byId = new Map(rows.map((r, i) => [r.id, i]));
  const rowById = new Map(rows.map((r) => [r.id, r]));
  const active = [];       // lane slot -> expected commit id (or null)
  const laneOf = new Array(rows.length).fill(0);
  const primePath = new Set();

  if (primeId && rowById.has(primeId)) {
    const visit = (id) => {
      if (primePath.has(id)) return;
      const row = rowById.get(id);
      if (!row) return;
      primePath.add(id);
      for (const parent of row.parents || []) visit(parent);
    };
    visit(primeId);
  } else {
    primeId = null;
    for (const row of rows) if (row.onCurrentPath || row.current) primePath.add(row.id);
  }

  rows.forEach((row, i) => {
    const waiting = [];
    active.forEach((id, l) => { if (id === row.id) waiting.push(l); });
    let lane;
    if (waiting.length) {
      lane = Math.min(...waiting);
      for (const l of waiting) active[l] = null;
    } else {
      lane = active.indexOf(null);
      if (lane === -1) { lane = active.length; active.push(null); }
    }
    laneOf[i] = lane;
    const parents = (row.parents || []).filter((p) => byId.has(p));
    if (parents.length) {
      active[lane] = parents[0];
      for (const p of parents.slice(1)) {
        if (!active.includes(p)) {
          let free = active.indexOf(null);
          if (free === -1) { free = active.length; active.push(p); }
          else active[free] = p;
        }
      }
    }
  });

  nodes = rows.map((row, i) => {
    const prime = primePath.has(row.id);
    const lane = prime ? 0 : laneOf[i] + 1;
    return {
      id: row.id,
      label: row.label || row.subject || row.id,
      time: shortTime(row.time),
      current: !!row.current,
      onCurrentPath: !!row.onCurrentPath,
      prime,
      parents: (row.parents || []).filter((p) => byId.has(p)),
      lane,
      x: PAD + lane * LANE_W,
      y: PAD + i * ROW_H,
      color: PALETTE[lane % PALETTE.length],
    };
  });
}

function shortTime(t) {
  // "HH:MM dd/mm/YYYY" -> "HH:MM dd/mm/yy"
  return String(t || "").replace(/(\\d{2}\\/\\d{2}\\/)\\d{2}(\\d{2})$/, "$1$2");
}

function draw() {
  const canvas = document.getElementById("c");
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const maxLane = nodes.reduce((m, n) => Math.max(m, n.lane), 0);
  const graphW = PAD * 2 + (maxLane + 1) * LANE_W + 140;
  const viewportW = Math.ceil(
    document.documentElement.clientWidth ||
    document.body.clientWidth ||
    window.innerWidth ||
    0
  );
  const cssW = Math.max(graphW, viewportW, 160);
  const cssH = nodes.length ? PAD * 2 + (nodes.length - 1) * ROW_H + R * 2 : 40;
  const dpr = window.devicePixelRatio || 1;
  canvas.width = cssW * dpr;
  canvas.height = cssH * dpr;
  canvas.style.width = cssW + "px";
  canvas.style.height = cssH + "px";
  const ctx = canvas.getContext("2d");
  ctx.scale(dpr, dpr);
  ctx.clearRect(0, 0, cssW, cssH);
  ctx.lineWidth = 2.5;
  ctx.lineCap = "round";

  const edgePath = (n, p) => {
    ctx.beginPath();
    ctx.moveTo(n.x, n.y);
    if (p.x === n.x) {
      ctx.lineTo(p.x, p.y);
    } else {
      const bend = Math.min(p.y - ROW_H * 0.8, p.y - 12);
      ctx.lineTo(n.x, bend);
      ctx.bezierCurveTo(n.x, p.y - 4, (n.x + p.x) / 2, p.y, p.x, p.y);
    }
  };

  // Edges first (child -> parent), colored by the child's lane.
  for (const n of nodes) {
    for (const pid of n.parents) {
      const p = byId.get(pid);
      if (!p) continue;
      const isSelectedEdge =
        selected && selected.after === n.id && selected.before === pid;
      const isPrimeEdge = n.prime && p.prime;
      ctx.strokeStyle = isSelectedEdge ? SEL_EDGE : isPrimeEdge ? CURRENT_EDGE : ALT_EDGE;
      ctx.lineWidth = isSelectedEdge ? 3.5 : 2.5;
      ctx.globalAlpha = isSelectedEdge ? 1 : isPrimeEdge ? 0.95 : 0.45;
      edgePath(n, p);
      ctx.stroke();
    }
  }
  ctx.globalAlpha = 1;
  ctx.lineWidth = 2.5;

  // Nodes on top. Prime nodes are blue, alternatives are gray. Selection
  // recolors the pressed anchor green and its parent red; hover enlarges.
  for (const n of nodes) {
    const isAfter = selected && selected.after === n.id;
    const isBefore = selected && selected.before === n.id;
    const isHover = hoverId === n.id;
    const color = isBefore ? SEL_RED : isAfter ? SEL_GREEN : n.prime ? CURRENT_EDGE : ALT_EDGE;
    const r = R + (isHover ? 2 : 0) + (isAfter || isBefore ? 1 : 0);

    ctx.globalAlpha = n.prime || n.current || isAfter || isBefore ? 1 : 0.55;
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.arc(n.x, n.y, r, 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.globalAlpha = 1;
}

function hit(ev) {
  const rect = document.getElementById("c").getBoundingClientRect();
  const x = ev.clientX - rect.left, y = ev.clientY - rect.top;
  return nodes.find((n) => (n.x - x) ** 2 + (n.y - y) ** 2 <= (R + 4) ** 2);
}

const canvas = document.getElementById("c");
const tip = document.getElementById("tip");

function showTip(n, ev) {
  tip.innerHTML =
    "<div>" + escapeHtml(n.label) + "</div>" +
    "<div class='time'>" + escapeHtml(n.time) + "</div>";
  tip.style.display = "block";
  const pad = 12;
  tip.style.left = Math.min(ev.clientX + pad, window.innerWidth - tip.offsetWidth - 4) + "px";
  tip.style.top = (ev.clientY + pad) + "px";
}

function hideTip() {
  tip.style.display = "none";
}

canvas.addEventListener("mousemove", (ev) => {
  const n = hit(ev);
  const newHover = n ? n.id : null;
  if (newHover !== hoverId) {
    hoverId = newHover;
    draw();
  }
  if (n) {
    canvas.style.cursor = "pointer";
    showTip(n, ev);
  } else {
    canvas.style.cursor = "default";
    hideTip();
  }
});
canvas.addEventListener("mouseleave", () => {
  hideTip();
  if (hoverId) { hoverId = null; draw(); }
});
canvas.addEventListener("click", (ev) => {
  const n = hit(ev);
  if (n && !n.current) {
    selected = { after: n.id, before: n.parents[0] || null };
    draw();
    vscode.postMessage({ type: "select", id: n.id });
  }
});
window.addEventListener("resize", () => draw());

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

window.addEventListener("message", (ev) => {
  const msg = ev.data;
  const msgEl = document.getElementById("msg");
  if (msg.type === "model") {
    msgEl.textContent = "";
    const firstLoad = !nodes.length;
    modelRows = (msg.model && msg.model.rows) || [];
    layout(msg.model);
    draw();
    if (!nodes.length) msgEl.textContent = "(no anchors yet)";
    if (firstLoad) window.scrollTo(0, document.body.scrollHeight);
  } else if (msg.type === "prime") {
    primeId = msg.id;
    layout({ rows: modelRows });
    draw();
  } else if (msg.type === "error") {
    msgEl.textContent = msg.message;
  }
});
vscode.postMessage({ type: "ready" });
</script>
</body>
</html>`;
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

  let timelineSelection = null;
  const setTimelineSelection = (id) => {
    timelineSelection = id || null;
    vscode.commands.executeCommand("setContext", "flashpoint.timelineAnchorSelected", !!timelineSelection);
  };

  async function revealTimelinePair(after, before) {
    try { await vscode.commands.executeCommand("workbench.actions.treeView.flashpoint.tree.collapseAll"); } catch { /* command varies by host */ }
    for (const id of [before, after].filter(Boolean)) {
      const item = provider._ckptItems.get(id);
      if (!item) continue;
      try {
        await tree.reveal(item, { expand: true, select: id === after, focus: false });
      } catch {
        // Off-stage anchors may not be materialized in the current tree.
      }
    }
  }

  // Timeline canvas: click a node → diff vs parent + expand in the tree.
  const timeline = new TimelineWebviewProvider(async (id) => {
    setTimelineSelection(id);
    const before = await openAnchorChanges(id, provider);
    await revealTimelinePair(id, before);
  });
  context.subscriptions.push(
    vscode.window.registerWebviewViewProvider("flashpoint.timeline", timeline)
  );

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
  cmd("flashpoint.refresh", () => { provider.refresh(); timeline.update(); });
  cmd("flashpoint.anchor", () =>
    runUserCommand(["anchor", "--speedster", "human", "--session", "human"], "Anchored.", provider));
  cmd("flashpoint.timetravel", (item) => timetravelWithPreview(item, provider));
  cmd("flashpoint.timelinePrime", () => {
    if (timelineSelection) timeline.makePrime(timelineSelection);
  });
  cmd("flashpoint.timelineTravel", async () => {
    if (!timelineSelection) return;
    await timetravelWithPreview({ hash: timelineSelection }, provider);
    provider.refresh();
    timeline.update();
  });
  cmd("flashpoint.openDiff", (item) => openDiff(item, provider));
  cmd("flashpoint.compareCurrent", (item) => compareCurrent(item));
  cmd("flashpoint.status", () => showStatusDoc());
  cmd("flashpoint.recheck", () => refreshPrereqs());
  // Setup wizard (jjckpt-vscode flow): binary check -> agent tick list ->
  // `fp setup --agents ... --yes`. Lives in setup.js.
  const afterSetup = () => {
    resolvedBin = null; // a fresh install may resolve differently now
    refreshPrereqs();
  };
  cmd("flashpoint.setup", () => setup.runSetup(context, resolveBin, afterSetup));
  cmd("flashpoint.check", async () => {
    if (await setup.showCheck(resolveBin)) setup.runSetup(context, resolveBin, afterSetup);
  });

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
    if (pollInFlight || (!tree.visible && !timeline.visible)) return;
    pollInFlight = true;
    try {
      const tip = (await run(["_tip"])).trim();
      if (tip && tip !== lastTip) {
        lastTip = tip;
        provider.refresh();
        timeline.update();
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
    return ok;
  }
  refreshPrereqs().then(() => setup.maybePrompt(context, resolveBin, afterSetup));

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
