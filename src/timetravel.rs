//! `fp timetravel <id>`: put files back exactly as they were at an anchor.
//! Never forks by itself — the working copy just becomes an empty commit on
//! top of the target; jj-lib abandons it again if it's left unchanged.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{bail, Result};
use pollster::block_on;

use crate::diff;
use crate::meta::{now_stamp, AnchorMeta};
use crate::repo::Fp;

/// Age in minutes after which a turn-active marker is considered stale
/// (a crashed speedster must not wedge timetravel forever).
const TURN_ACTIVE_TTL_MINUTES: u64 = 30;

pub fn run(fp: &mut Fp, id: &str, yes: bool) -> Result<()> {
    let target = crate::ids::resolve(fp, id)?;

    warn_if_turn_active(fp)?;

    // Safety rule: timetravel must never destroy work. Seal un-anchored
    // changes automatically before rewriting anything.
    let safety = AnchorMeta {
        title: format!("safety anchor (before timetravel) {}", now_stamp()),
        session: "auto".into(),
        speedster: "human".into(),
        phase: "auto".into(),
        base: crate::meta::git_head(fp.workspace.workspace_root()),
    };
    if let Some(sealed) = fp.seal_anchor(&safety.to_description())? {
        println!(
            "sealed safety anchor {} (your un-anchored changes are safe)",
            crate::log::short_id(&sealed)
        );
    }

    let wc = fp.wc_commit()?;
    if wc.parent_ids() == [target.id().clone()] {
        println!("already at {id} — nothing to do");
        return Ok(());
    }

    // Confirm with a changed-file count before rewriting the workspace.
    let changes = diff::entries(&wc.tree(), &target.tree())?;
    if !yes {
        let n = changes.len();
        eprint!(
            "timetravel to {id} rewrites {n} file{}. continue? [y/N] ",
            if n == 1 { "" } else { "s" }
        );
        std::io::stderr().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            bail!("timetravel cancelled");
        }
    }

    // Safe-zone: capture the on-disk state of protected paths that the
    // checkout would rewrite, and put them back afterwards.
    let safe = safe_zone_snapshot(fp, &changes)?;

    // Repo side: working copy becomes a fresh empty commit on the target
    // (the lazy-fork seed). The empty commit we leave behind is abandoned
    // by jj-lib if it was discardable.
    let mut tx = fp.repo.start_transaction();
    let new_wc = block_on(
        tx.repo_mut()
            .check_out(fp.workspace_name.clone(), &target),
    )?;
    block_on(tx.repo_mut().rebase_descendants())?;
    fp.repo = block_on(tx.commit(format!("timetravel to {id}")))?;

    // File side: rewrite the workspace to the target tree.
    let old_tree = wc.tree();
    let stats = block_on(fp.workspace.check_out(
        fp.repo.op_id().clone(),
        Some(&old_tree),
        &new_wc,
    ))?;

    let kept = restore_safe_zone(fp, safe)?;

    println!(
        "timetraveled to {id}: {} file{} updated{}",
        stats.updated_files + stats.added_files + stats.removed_files,
        if stats.updated_files + stats.added_files + stats.removed_files == 1 { "" } else { "s" },
        if kept > 0 {
            format!(", {kept} safe-zone file{} kept", if kept == 1 { "" } else { "s" })
        } else {
            String::new()
        }
    );
    println!("change something and your next anchor becomes a flashpoint (new timeline)");
    Ok(())
}

fn warn_if_turn_active(fp: &Fp) -> Result<()> {
    let marker = fp.workspace.workspace_root().join(".fp/turn-active");
    let Ok(meta) = std::fs::metadata(&marker) else {
        return Ok(());
    };
    let fresh = meta
        .modified()
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age.as_secs() < TURN_ACTIVE_TTL_MINUTES * 60);
    if fresh {
        bail!(
            "a speedster is mid-turn here (.fp/turn-active is fresh) — wait for the turn to \
             finish, or delete the marker and re-run if you're sure"
        );
    }
    Ok(())
}

type SafeSnapshot = HashMap<PathBuf, Option<Vec<u8>>>;

/// Belt-and-braces for stores where a safe-zone path was tracked before the
/// safe list covered it: preserve the on-disk state across the checkout.
fn safe_zone_snapshot(fp: &Fp, changes: &[diff::Entry]) -> Result<SafeSnapshot> {
    let globs = crate::safezone::load(fp.workspace.workspace_root())?.globset;
    let root = fp.workspace.workspace_root();
    let mut snapshot = HashMap::new();
    for entry in changes {
        if globs.is_match(&entry.path) {
            let disk = root.join(&entry.path);
            let content = std::fs::read(&disk).ok();
            snapshot.insert(disk, content);
        }
    }
    Ok(snapshot)
}

fn restore_safe_zone(_fp: &Fp, snapshot: SafeSnapshot) -> Result<usize> {
    let n = snapshot.len();
    for (path, content) in snapshot {
        match content {
            Some(bytes) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, bytes)?;
            }
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    Ok(n)
}
