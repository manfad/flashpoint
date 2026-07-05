//! `fp setup` — wire speedster hook configs and put `fp` on PATH.
//!
//! Flow (ported from jjckpt's proven setup wizard, `~/jjckpt-vscode/setup.js`):
//!   1. Copy this binary to a stable per-user location and make sure that
//!      location is on PATH (shell rc on macOS/Linux, user registry on
//!      Windows). Hook configs reference the stable ABSOLUTE path, so hooks
//!      work even in GUI-launched agents that never see the shell PATH.
//!   2. Keep `.jj/` and `.fp/` out of git everywhere via the global excludes.
//!   3. Tick-list of speedsters (detected ones pre-ticked) → write each one's
//!      native hook config: pre-turn Human safepoint + post-turn anchor.
//!
//! Existing configs are merged, never clobbered: unrelated hook entries are
//! kept, stale flashpoint/jjckpt entries are replaced, and every file we touch
//! gets a `.backup` copy first. Running both jjckpt and flashpoint hooks would
//! double-anchor the same `.jj` store, so jjckpt entries are retired here.

use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::{json, Value};

struct Speedster {
    slug: &'static str,
    label: &'static str,
    /// Existence of this dir (under home) means the agent is installed.
    detect_rel: &'static str,
}

const SPEEDSTERS: &[Speedster] = &[
    Speedster { slug: "claude-code", label: "Claude Code", detect_rel: ".claude" },
    Speedster { slug: "codex", label: "Codex", detect_rel: ".codex" },
    Speedster { slug: "cursor", label: "Cursor", detect_rel: ".cursor" },
    Speedster { slug: "gemini", label: "Gemini", detect_rel: ".gemini" },
    Speedster { slug: "antigravity", label: "Antigravity", detect_rel: ".gemini/antigravity-cli" },
    Speedster { slug: "opencode", label: "OpenCode", detect_rel: ".config/opencode" },
    Speedster { slug: "vscode-copilot", label: "VS Code Copilot", detect_rel: ".copilot" },
];

/// Any hook entry containing one of these is ours (or jjckpt's, which
/// flashpoint supersedes) and gets replaced on re-run.
const OWNED_MARKERS: &[&str] = &["hook --speedster", "hook-commit", "jjckpt", "flashpoint"];

/// `fp setup --check`: read-only report of what setup would fix. Exits
/// nonzero when something needs attention, so it can gate scripts/CI.
pub fn check() -> Result<()> {
    let home = home_dir()?;
    let mut problems = 0;

    let dir = install_dir(&home);
    let target = dir.join(if cfg!(windows) { "fp.exe" } else { "fp" });
    if target.exists() {
        println!("binary    ok      {}", target.display());
    } else {
        problems += 1;
        println!("binary    MISSING {} — run `fp setup`", target.display());
    }
    if on_path(&dir) {
        println!("PATH      ok      {}", dir.display());
    } else {
        problems += 1;
        println!("PATH      MISSING {} not on PATH — run `fp setup`", dir.display());
    }
    match excludes_missing(&home) {
        missing if missing.is_empty() => println!("gitignore ok      .jj/ + .fp/ globally excluded"),
        missing => {
            problems += 1;
            println!("gitignore MISSING {} not in global excludes — run `fp setup`", missing.join(" + "));
        }
    }

    println!("\nspeedster hooks:");
    for s in SPEEDSTERS {
        let detected = home.join(s.detect_rel).exists();
        let state = hook_state(s.slug, &home);
        let line = match &state {
            HookState::Wired(exe) => format!("wired ({})", exe.display()),
            HookState::Partial => "PARTIAL — one of pre/post missing; re-run `fp setup`".into(),
            HookState::StaleBinary(exe) => {
                format!("STALE — hook binary missing ({}); re-run `fp setup`", exe.display())
            }
            HookState::Jjckpt => "JJCKPT — old jjckpt hooks still active; `fp setup` retires them".into(),
            HookState::NotWired if detected => {
                format!("not wired (detected — `fp setup --agents {}`)", s.slug)
            }
            HookState::NotWired => "not wired (not detected)".into(),
        };
        let needs_attention = matches!(
            state,
            HookState::Partial | HookState::StaleBinary(_) | HookState::Jjckpt
        ) || (detected && matches!(state, HookState::NotWired));
        if needs_attention {
            problems += 1;
        }
        println!("  {:<16} {}", s.label, line);
    }

    if problems > 0 {
        println!("\n{problems} item(s) need attention — run `fp setup` to fix.");
        std::process::exit(1);
    }
    println!("\nAll good.");
    Ok(())
}

pub fn run(agents: Vec<String>, all: bool, yes: bool, no_path: bool) -> Result<()> {
    let home = home_dir()?;
    let interactive = agents.is_empty() && !all && std::io::stdin().is_terminal();

    // 1) Stable binary + PATH ------------------------------------------------
    let exe = if no_path {
        std::env::current_exe()?
    } else {
        let exe = install_binary(&home)?;
        ensure_on_path(&home, exe.parent().expect("install dir"), yes, interactive);
        exe
    };

    // 2) Global git excludes -------------------------------------------------
    match ensure_global_excludes(&home) {
        Ok(Some(file)) => println!("gitignore: .jj/ and .fp/ excluded globally ({})", file.display()),
        Ok(None) => println!("gitignore: .jj/ and .fp/ already excluded globally"),
        Err(e) => println!("gitignore: skipped ({e})"),
    }

    // 3) Which speedsters? ---------------------------------------------------
    let detected: Vec<&Speedster> = SPEEDSTERS
        .iter()
        .filter(|s| home.join(s.detect_rel).exists())
        .collect();
    let picked: Vec<&Speedster> = if all {
        SPEEDSTERS.iter().collect()
    } else if !agents.is_empty() {
        let mut picked = Vec::new();
        for slug in &agents {
            let s = SPEEDSTERS
                .iter()
                .find(|s| s.slug == slug)
                .with_context(|| {
                    let known: Vec<&str> = SPEEDSTERS.iter().map(|s| s.slug).collect();
                    format!("unknown speedster '{slug}' (known: {})", known.join(", "))
                })?;
            picked.push(s);
        }
        picked
    } else if interactive {
        pick_speedsters(&detected, &home)?
    } else {
        // Non-interactive with no explicit list: wire what's detected.
        detected.clone()
    };

    if picked.is_empty() {
        println!("hooks: none selected — anchors via `fp anchor` only");
        return Ok(());
    }

    // 4) Wire hooks ----------------------------------------------------------
    for s in &picked {
        match install_hooks(s.slug, &home, &exe) {
            Ok(file) => println!("hooks: {} wired ({})", s.label, file.display()),
            Err(e) => println!("hooks: {} FAILED — {e}", s.label),
        }
    }
    println!("\nDone. Anchors start on each speedster's next turn; check with `fp log`.");
    Ok(())
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("cannot locate the home directory (HOME/USERPROFILE unset)")
}

// ---- stable binary + PATH ---------------------------------------------------

fn install_dir(home: &Path) -> PathBuf {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Local"))
            .join("flashpoint")
            .join("bin")
    } else {
        home.join(".local").join("bin")
    }
}

/// Copy the running binary to the stable per-user dir. Hooks point at this
/// path, so an `npm upgrade` won't silently break them — re-run `fp setup`
/// after upgrading to refresh the copy.
fn install_binary(home: &Path) -> Result<PathBuf> {
    let dir = install_dir(home);
    let target = dir.join(if cfg!(windows) { "fp.exe" } else { "fp" });
    let current = std::env::current_exe()?;

    let same = target.exists()
        && current.canonicalize().ok() == target.canonicalize().ok();
    if same {
        println!("binary: already running from {}", target.display());
        return Ok(target);
    }

    std::fs::create_dir_all(&dir)?;
    std::fs::copy(&current, &target)
        .with_context(|| format!("copying {} -> {}", current.display(), target.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))?;
    }
    println!("binary: installed to {}", target.display());
    Ok(target)
}

fn on_path(dir: &Path) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&path_var).any(|p| {
        if cfg!(windows) {
            p.to_string_lossy().trim_end_matches(['\\', '/']).eq_ignore_ascii_case(
                dir.to_string_lossy().trim_end_matches(['\\', '/']),
            )
        } else {
            p == dir
        }
    })
}

fn ensure_on_path(home: &Path, dir: &Path, yes: bool, interactive: bool) {
    if on_path(dir) {
        println!("PATH: {} already on PATH", dir.display());
        return;
    }
    let what = if cfg!(windows) {
        format!("add {} to your user PATH (registry)", dir.display())
    } else {
        format!("add {} to PATH in {}", dir.display(), shell_rc(home).display())
    };
    if !yes && interactive && !confirm(&format!("PATH: {what}?")) {
        println!("PATH: skipped — hooks still work (they use the absolute path)");
        return;
    }
    let result = if cfg!(windows) { add_path_windows(dir) } else { add_path_unix(home) };
    match result {
        Ok(msg) => println!("PATH: {msg} — restart your terminal for `fp` to resolve"),
        Err(e) => println!("PATH: FAILED ({e}) — add {} to PATH manually", dir.display()),
    }
}

/// The rc file for the user's login shell (macOS defaults to zsh).
fn shell_rc(home: &Path) -> PathBuf {
    let shell = std::env::var("SHELL").unwrap_or_default();
    let name = shell.rsplit('/').next().unwrap_or("");
    match name {
        "bash" => home.join(".bashrc"),
        "fish" => home.join(".config").join("fish").join("config.fish"),
        "zsh" => home.join(".zshrc"),
        _ if cfg!(target_os = "macos") => home.join(".zshrc"),
        _ => home.join(".profile"),
    }
}

fn add_path_unix(home: &Path) -> Result<String> {
    let rc = shell_rc(home);
    let current = std::fs::read_to_string(&rc).unwrap_or_default();
    // `.local/bin` covers both our literal line and any hand-written variant.
    if current.contains(".local/bin") {
        return Ok(format!("{} already configures it", rc.display()));
    }
    let line = if rc.ends_with("config.fish") {
        "fish_add_path -g $HOME/.local/bin"
    } else {
        "export PATH=\"$HOME/.local/bin:$PATH\""
    };
    if let Some(parent) = rc.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let prefix = if current.is_empty() || current.ends_with('\n') { "" } else { "\n" };
    std::fs::write(&rc, format!("{current}{prefix}\n# flashpoint\n{line}\n"))?;
    Ok(format!("appended to {}", rc.display()))
}

/// Append to the *user* Path via the registry (survives reboots, no admin).
/// `setx` is avoided on purpose: it truncates PATH at 1024 chars.
fn add_path_windows(dir: &Path) -> Result<String> {
    let dir_ps = dir.to_string_lossy().replace('\'', "''");
    let script = format!(
        "$d = '{dir_ps}'; \
         $p = [Environment]::GetEnvironmentVariable('Path', 'User'); \
         if (($p -split ';') -notcontains $d) {{ \
           [Environment]::SetEnvironmentVariable('Path', ($p.TrimEnd(';') + ';' + $d), 'User') \
         }}"
    );
    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()
        .context("running powershell")?;
    anyhow::ensure!(status.success(), "powershell exited with {status}");
    Ok(format!("{} added to the user PATH", dir.display()))
}

// ---- global git excludes ----------------------------------------------------

/// Resolve the global git excludes file: `core.excludesFile` if configured,
/// else git's default `$XDG_CONFIG_HOME/git/ignore`.
fn global_excludes_path(home: &Path) -> PathBuf {
    let configured = Command::new("git")
        .args(["config", "--global", "core.excludesFile"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(p) = configured {
        let p = p
            .strip_prefix("~/")
            .map(|rest| home.join(rest).to_string_lossy().into_owned())
            .unwrap_or(p);
        let p = PathBuf::from(p);
        return if p.is_absolute() { p } else { home.join(p) };
    }
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    xdg.join("git").join("ignore")
}

fn excludes_missing(home: &Path) -> Vec<&'static str> {
    let current = std::fs::read_to_string(global_excludes_path(home)).unwrap_or_default();
    let has = |entry: &str| {
        current
            .lines()
            .map(str::trim)
            .any(|l| l == entry || l == entry.trim_end_matches('/'))
    };
    [".jj/", ".fp/"].into_iter().filter(|e| !has(e)).collect()
}

/// Returns the file path when something was added, None when already covered.
fn ensure_global_excludes(home: &Path) -> Result<Option<PathBuf>> {
    let file = global_excludes_path(home);
    let current = std::fs::read_to_string(&file).unwrap_or_default();
    let missing = excludes_missing(home);
    if missing.is_empty() {
        return Ok(None);
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let prefix = if current.is_empty() || current.ends_with('\n') { "" } else { "\n" };
    std::fs::write(&file, format!("{current}{prefix}{}\n", missing.join("\n")))?;
    Ok(Some(file))
}

// ---- interaction -------------------------------------------------------------

fn confirm(prompt: &str) -> bool {
    dialoguer::Confirm::with_theme(&dialoguer::theme::ColorfulTheme::default())
        .with_prompt(prompt)
        .default(true)
        .interact()
        .unwrap_or(false)
}

/// Vite-style tick list: ↑/↓ to move, space to toggle, enter to confirm.
/// Detected speedsters start ticked; each row shows its current hook state.
fn pick_speedsters<'a>(detected: &[&'a Speedster], home: &Path) -> Result<Vec<&'a Speedster>> {
    let items: Vec<String> = SPEEDSTERS
        .iter()
        .map(|s| {
            let mut marks: Vec<&str> = Vec::new();
            if detected.iter().any(|d| d.slug == s.slug) {
                marks.push("detected");
            }
            match hook_state(s.slug, home) {
                HookState::Wired(_) => marks.push("already wired"),
                HookState::Partial => marks.push("partially wired"),
                HookState::StaleBinary(_) => marks.push("stale hooks"),
                HookState::Jjckpt => marks.push("jjckpt hooks"),
                HookState::NotWired => {}
            }
            if marks.is_empty() {
                s.label.to_string()
            } else {
                format!("{:<16} {}", s.label, marks.join(" · "))
            }
        })
        .collect();
    let defaults: Vec<bool> = SPEEDSTERS
        .iter()
        .map(|s| detected.iter().any(|d| d.slug == s.slug))
        .collect();

    let picked = dialoguer::MultiSelect::with_theme(&dialoguer::theme::ColorfulTheme::default())
        .with_prompt("Which speedsters (agents) do you use? (space = toggle, enter = confirm)")
        .items(&items)
        .defaults(&defaults)
        .report(false)
        .interact_opt()
        .context("reading the speedster selection")?
        .unwrap_or_default(); // Esc = wire nothing

    Ok(picked.into_iter().map(|i| &SPEEDSTERS[i]).collect())
}

// ---- hook state inspection ----------------------------------------------------

enum HookState {
    /// Both phases wired and the embedded binary exists.
    Wired(PathBuf),
    /// Only one of pre/post is wired.
    Partial,
    /// Wired, but the binary the hooks point at is gone (moved/uninstalled).
    StaleBinary(PathBuf),
    /// No flashpoint hooks, but jjckpt's are still active (would double-anchor).
    Jjckpt,
    NotWired,
}

/// The config file `install_hooks` writes for a speedster.
fn config_path(slug: &str, home: &Path) -> PathBuf {
    match slug {
        "claude-code" => home.join(".claude").join("settings.json"),
        "codex" => home.join(".codex").join("hooks.json"),
        "cursor" => home.join(".cursor").join("hooks.json"),
        "gemini" => home.join(".gemini").join("settings.json"),
        "antigravity" => home.join(".gemini").join("antigravity-cli").join("hooks.json"),
        "vscode-copilot" => home.join(".copilot").join("hooks").join("flashpoint.json"),
        "opencode" => home.join(".config").join("opencode").join("plugins").join("flashpoint.js"),
        other => unreachable!("unknown speedster '{other}'"),
    }
}

/// Every command-carrying string in a config: decoded JSON strings for the
/// JSON formats, the raw source for the OpenCode JS plugin.
fn command_texts(file: &Path) -> Vec<String> {
    let Ok(raw) = std::fs::read_to_string(file) else { return Vec::new() };
    if file.extension().is_some_and(|e| e == "js") {
        return vec![raw];
    }
    let Ok(json) = serde_json::from_str::<Value>(&raw) else { return Vec::new() };
    let mut out = Vec::new();
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Array(a) => a.iter().for_each(|v| walk(v, out)),
            Value::Object(o) => o.values().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    walk(&json, &mut out);
    out
}

/// The binary path preceding `needle_pos` in a hook command.
///
/// Supports the direct form (`"<exe>" hook ...`) and Windows best-effort
/// wrappers (`cmd /C ""<exe>" hook ... || exit /B 0"`).
fn embedded_exe(text: &str, needle_pos: usize) -> Option<PathBuf> {
    let before = text[..needle_pos].trim_end();
    if let Some(closing) = before.strip_suffix('"') {
        if let Some(start) = closing.rfind('"') {
            return Some(PathBuf::from(&closing[start + 1..]));
        }
    }

    #[cfg(windows)]
    {
        if let Some(exe_end) = before.to_ascii_lowercase().rfind(".exe") {
            let exe_end = exe_end + ".exe".len();
            let prefix = &before[..exe_end];
            let start = prefix
                .rfind('"')
                .or_else(|| prefix.rfind(' '))
                .map_or(0, |i| i + 1);
            let path = prefix[start..].trim_matches('"');
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }
    }

    None
}

fn hook_state(slug: &str, home: &Path) -> HookState {
    let file = config_path(slug, home);
    let texts = command_texts(&file);
    let find = |phase: &str| {
        let needle = format!("hook --speedster {slug} --phase {phase}");
        texts.iter().find_map(|t| t.find(&needle).map(|pos| embedded_exe(t, pos)))
    };
    let (pre, post) = (find("pre"), find("post"));
    let jjckpt_active = texts.iter().any(|t| t.contains("jjckpt"))
        || (slug == "vscode-copilot" && home.join(".copilot/hooks/jjckpt.json").exists())
        || (slug == "opencode" && home.join(".config/opencode/plugins/jjckpt.js").exists());

    if jjckpt_active {
        return HookState::Jjckpt;
    }
    match (pre, post) {
        (Some(pre_exe), Some(post_exe)) => {
            let exes: Vec<PathBuf> = [pre_exe, post_exe].into_iter().flatten().collect();
            if let Some(missing) = exes.iter().find(|e| !e.exists()) {
                return HookState::StaleBinary(missing.clone());
            }
            HookState::Wired(exes.into_iter().next().unwrap_or(file))
        }
        (None, None) => HookState::NotWired,
        _ => HookState::Partial,
    }
}

// ---- hook wiring -------------------------------------------------------------

fn owned(v: &Value) -> bool {
    let text = v.to_string();
    OWNED_MARKERS.iter().any(|m| text.contains(m))
}

/// Merge-write a JSON config: parse (or start fresh), back up, mutate, pretty-print.
fn merge_json(file: &Path, mutate: impl FnOnce(&mut Value)) -> Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut obj = std::fs::read_to_string(file)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));
    if !obj.is_object() {
        obj = json!({});
    }
    if file.exists() {
        std::fs::copy(file, file.with_extension("json.backup"))?; // never clobber without a backup
    }
    mutate(&mut obj);
    std::fs::write(file, format!("{}\n", serde_json::to_string_pretty(&obj)?))?;
    Ok(())
}

/// Replace our entries in `obj[..path][event]` while keeping foreign ones.
fn upsert(obj: &mut Value, event: &str, entry: Value) {
    let hooks = obj
        .as_object_mut()
        .expect("merge_json guarantees an object")
        .entry("hooks")
        .or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let arr = hooks
        .as_object_mut()
        .expect("just ensured object")
        .entry(event)
        .or_insert_with(|| json!([]));
    if !arr.is_array() {
        *arr = json!([]);
    }
    let list = arr.as_array_mut().expect("just ensured array");
    list.retain(|e| !owned(e));
    list.push(entry);
}

/// Disable a leftover jjckpt file by renaming it — running jjckpt hooks next
/// to flashpoint would double-anchor the same `.jj` store.
fn retire_jjckpt_file(file: &Path) {
    if file.exists() {
        let backup = file.with_file_name(format!(
            "{}.retired-by-flashpoint",
            file.file_name().unwrap_or_default().to_string_lossy()
        ));
        if std::fs::rename(file, &backup).is_ok() {
            println!("hooks: retired jjckpt config {} (kept as .retired-by-flashpoint)", file.display());
        }
    }
}

/// Wire one speedster's hooks in its native config format; returns the file touched.
fn install_hooks(slug: &str, home: &Path, exe: &Path) -> Result<PathBuf> {
    let cmd = format!("\"{}\"", exe.display());
    let pre = format!("{cmd} hook --speedster {slug} --phase pre");
    let post = format!("{cmd} hook --speedster {slug} --phase post");

    let file = match slug {
        "claude-code" => {
            let file = home.join(".claude").join("settings.json");
            merge_json(&file, |j| {
                upsert(j, "Stop", json!({ "hooks": [{ "type": "command", "command": format!("{post} || true") }] }));
                upsert(j, "UserPromptSubmit", json!({ "hooks": [{ "type": "command", "command": format!("{pre} || true") }] }));
            })?;
            file
        }
        "codex" => {
            let file = home.join(".codex").join("hooks.json");
            merge_json(&file, |j| {
                upsert(j, "Stop", json!({ "hooks": [{ "type": "command", "command": best_effort(&post), "statusMessage": "Saving anchor" }] }));
                // Payload carries prompt + session_id + cwd (same field names as Claude Code).
                upsert(j, "UserPromptSubmit", json!({ "hooks": [{ "type": "command", "command": best_effort(&pre), "statusMessage": "Saving safepoint", "timeout": 30 }] }));
            })?;
            file
        }
        "cursor" => {
            let file = home.join(".cursor").join("hooks.json");
            merge_json(&file, |j| {
                let map = j.as_object_mut().expect("object");
                map.entry("version").or_insert(json!(1));
                upsert(j, "stop", json!({ "command": post, "timeout": 10 }));
                // beforeSubmitPrompt payload carries conversation_id + prompt, which
                // `fp hook` parses for the Human safepoint and the anchor title.
                upsert(j, "beforeSubmitPrompt", json!({ "command": pre, "timeout": 10 }));
            })?;
            file
        }
        "gemini" => {
            let file = home.join(".gemini").join("settings.json");
            merge_json(&file, |j| {
                upsert(j, "AfterAgent", json!({ "hooks": [{ "type": "command", "command": post, "name": "flashpoint-anchor", "timeout": 10000 }] }));
                // BeforeAgent payload carries session_id + cwd + prompt (Claude Code
                // field names), so `fp hook` parses it as-is.
                upsert(j, "BeforeAgent", json!({ "hooks": [{ "type": "command", "command": pre, "name": "flashpoint-safepoint", "timeout": 10000 }] }));
            })?;
            file
        }
        "antigravity" => {
            // Top-level is keyed by named hook groups: add ours, drop jjckpt's, keep the rest.
            let file = home.join(".gemini").join("antigravity-cli").join("hooks.json");
            merge_json(&file, |j| {
                let map = j.as_object_mut().expect("object");
                map.retain(|k, v| !k.contains("jjckpt") && !owned(v));
                // Antigravity has no UserPromptSubmit; PreInvocation fires before the
                // agent runs. Its payload has session_id + cwd but NO prompt, so turns
                // get timestamp titles, not prompt-derived ones.
                map.insert(
                    "flashpoint".into(),
                    json!({
                        "Stop": [{ "hooks": [{ "type": "command", "command": post, "timeout": 10 }] }],
                        "PreInvocation": [{ "hooks": [{ "type": "command", "command": pre, "timeout": 10 }] }],
                    }),
                );
            })?;
            file
        }
        "vscode-copilot" => {
            // Copilot collects user-level hooks from every *.json in ~/.copilot/hooks/.
            // We own flashpoint.json; the Stop payload carries sessionId + cwd.
            retire_jjckpt_file(&home.join(".copilot").join("hooks").join("jjckpt.json"));
            let file = home.join(".copilot").join("hooks").join("flashpoint.json");
            merge_json(&file, |j| {
                upsert(j, "Stop", json!({ "type": "command", "command": post, "timeout": 10 }));
                upsert(j, "UserPromptSubmit", json!({ "type": "command", "command": pre, "timeout": 10 }));
            })?;
            file
        }
        "opencode" => {
            let dir = home.join(".config").join("opencode").join("plugins");
            retire_jjckpt_file(&dir.join("jjckpt.js"));
            std::fs::create_dir_all(&dir)?;
            let file = dir.join("flashpoint.js");
            if file.exists() {
                std::fs::copy(&file, file.with_extension("js.backup"))?;
            }
            std::fs::write(&file, opencode_plugin(&cmd))?;
            file
        }
        other => anyhow::bail!("no hook wiring for '{other}'"),
    };
    Ok(file)
}

/// Hooks are a safety net, not the user's primary command. Keep setup visible
/// through `fp check`, but don't let an anchor failure fail the agent turn.
fn best_effort(command: &str) -> String {
    if cfg!(windows) {
        format!("cmd /C \"{command} || exit /B 0\"")
    } else {
        format!("{command} || true")
    }
}

/// OpenCode has no hook config — it loads JS plugins. Ported from jjckpt:
/// user message submitted → pre (OpenCode has no pre-prompt hook, so we react
/// to message.updated with role user; the text lives in message.part.updated
/// events, which can arrive before OR after the message event — handle both
/// orders), session.idle → post.
fn opencode_plugin(cmd: &str) -> String {
    format!(
        r#"// flashpoint anchor plugin for OpenCode (auto-installed by `fp setup`).
export const FlashpointAnchor = async ({{ $, directory }}) => {{
  const seen = new Set(); // user message ids already safepointed
  const texts = new Map(); // messageID -> text (user AND assistant parts land here)
  const pre = async (sid, mid) => {{
    const payload = JSON.stringify({{ session_id: sid || "opencode", cwd: directory, prompt: texts.get(mid) || "" }});
    await $`printf %s ${{payload}} | {cmd} hook --speedster opencode --phase pre`.quiet().nothrow();
  }};
  return {{
    event: async ({{ event }}) => {{
      const p = event.properties || {{}};
      if (event.type === "message.updated" && p.info && p.info.role === "user") {{
        if (seen.has(p.info.id)) return;
        seen.add(p.info.id);
        await pre(p.info.sessionID, p.info.id);
      }} else if (event.type === "message.part.updated" && p.part && p.part.type === "text" && p.part.text) {{
        if (texts.size > 500) texts.clear(); // assistant stream parts land here too
        const stale = texts.get(p.part.messageID) === p.part.text;
        texts.set(p.part.messageID, p.part.text);
        // Text arrived after the safepoint: re-run pre — the working copy is
        // clean now, so it only refreshes the stashed title, commits nothing.
        if (seen.has(p.part.messageID) && !stale) await pre(p.part.sessionID, p.part.messageID);
      }} else if (event.type === "session.idle") {{
        const sid = p.sessionID || "opencode";
        const payload = JSON.stringify({{ session_id: sid, cwd: directory }});
        await $`printf %s ${{payload}} | {cmd} hook --speedster opencode --phase post`.quiet().nothrow();
      }}
    }},
  }};
}};
"#
    )
}
