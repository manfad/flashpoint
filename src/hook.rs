//! `fp hook --speedster <slug> --phase pre|post`: what agent hook configs
//! invoke. Reads the hook payload JSON from stdin (session id, cwd, prompt).
//!
//! pre  — seal the human's between-turn edits as a Human safepoint
//!        (diff-gated), warn/block if another speedster is mid-turn, then
//!        write the turn-active marker and stash the prompt for the title.
//! post — seal the turn's changes as the speedster's anchor and clear the
//!        marker.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::meta::{now_stamp, AnchorMeta};
use crate::repo::Fp;

const TURN_ACTIVE_TTL_MINUTES: u64 = 30;

struct Payload {
    session: String,
    cwd: Option<PathBuf>,
    prompt: Option<String>,
}

fn read_payload() -> Payload {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
    let get = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| json.get(k).and_then(|v| v.as_str()))
            .map(str::to_string)
    };
    Payload {
        session: get(&["session_id", "conversation_id", "sessionId", "thread_id"])
            .unwrap_or_else(|| "manual".to_string()),
        cwd: get(&["cwd", "workspace", "project_dir"]).map(PathBuf::from),
        prompt: get(&["prompt", "user_prompt", "message"]),
    }
}

/// Zero-token title: first clause of the prompt, capped at 8 words.
fn summarize(prompt: &str) -> Option<String> {
    let first = prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('<'))?;
    let clause = first
        .split(['.', '!', '?', ';'])
        .next()
        .unwrap_or(first)
        .trim();
    let words: Vec<&str> = clause.split_whitespace().take(8).collect();
    if words.is_empty() {
        return None;
    }
    let mut title = words.join(" ");
    if let Some(first_char) = title.get(..1) {
        title = first_char.to_uppercase() + &title[1..];
    }
    Some(title)
}

fn fp_dir(root: &Path) -> PathBuf {
    root.join(".fp")
}

fn marker_path(root: &Path) -> PathBuf {
    fp_dir(root).join("turn-active")
}

fn stash_path(root: &Path, session: &str) -> PathBuf {
    let safe: String = session
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    fp_dir(root).join(format!("prompt-{safe}"))
}

fn marker_is_fresh(root: &Path) -> Option<String> {
    let path = marker_path(root);
    let meta = std::fs::metadata(&path).ok()?;
    let fresh = meta
        .modified()
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age.as_secs() < TURN_ACTIVE_TTL_MINUTES * 60);
    fresh.then(|| std::fs::read_to_string(&path).unwrap_or_default().trim().to_string())
}

pub fn run(speedster: &str, phase: &str) -> Result<()> {
    let payload = read_payload();
    let cwd = match &payload.cwd {
        Some(dir) => dir.clone(),
        None => std::env::current_dir()?,
    };

    let mut fp = Fp::open_or_init(&cwd)?;
    let root = fp.workspace.workspace_root().to_path_buf();
    let git_base = crate::meta::git_head(&root);

    match phase {
        "pre" => {
            // Second-speedster guard: another agent's turn is still active.
            if let Some(owner) = marker_is_fresh(&root)
                && owner != payload.session
            {
                bail!(
                    "another speedster is mid-turn here — run this agent in its own \
                     worktree, or wait for the turn to finish"
                );
            }

            // Human safepoint for between-turn edits (diff-gated: no diff, no
            // anchor). All human edits group under one session.
            let meta = AnchorMeta {
                title: now_stamp(),
                session: "human".into(),
                speedster: "human".into(),
                phase: "pre".into(),
                base: git_base,
            };
            if let Some(sealed) = fp.seal_anchor(&meta.to_description())? {
                println!("safepoint: {}", crate::log::short_id(&sealed));
            }

            let _ = std::fs::create_dir_all(fp_dir(&root));
            let _ = std::fs::write(marker_path(&root), &payload.session);
            if let Some(title) = payload.prompt.as_deref().and_then(summarize) {
                let _ = std::fs::write(stash_path(&root, &payload.session), title);
            }
        }
        "post" => {
            let stash = stash_path(&root, &payload.session);
            let title = std::fs::read_to_string(&stash).ok();
            let _ = std::fs::remove_file(&stash);
            let meta = AnchorMeta {
                title: title.unwrap_or_else(now_stamp),
                session: payload.session.clone(),
                speedster: speedster.to_string(),
                phase: "post".into(),
                base: git_base,
            };
            if let Some(sealed) = fp.seal_anchor(&meta.to_description())? {
                println!("anchored: {}", crate::log::short_id(&sealed));
            }
            let _ = std::fs::remove_file(marker_path(&root));
        }
        other => bail!("unknown hook phase '{other}' (expected pre or post)"),
    }
    Ok(())
}
