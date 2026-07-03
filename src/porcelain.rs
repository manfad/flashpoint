//! Hidden porcelain commands consumed by UI surfaces (VS Code extension).
//! Output formats are the jjckpt compatibility contract — do not change them.

use anyhow::{Context, Result};
use futures::AsyncReadExt as _;
use jj_lib::backend::TreeValue;
use jj_lib::commit::Commit;
use jj_lib::object_id::ObjectId;
use jj_lib::repo::Repo;
use jj_lib::repo_path::RepoPath;
use pollster::block_on;
use serde::Serialize;

use crate::log::short_id;
use crate::meta::{git_head, AnchorMeta};
use crate::repo::Fp;

fn time_of(commit: &Commit) -> String {
    let ts = &commit.committer().timestamp;
    chrono::DateTime::from_timestamp_millis(ts.timestamp.0)
        .map(|t| {
            let tz = chrono::FixedOffset::east_opt(ts.tz_offset * 60)
                .unwrap_or_else(|| chrono::FixedOffset::east_opt(0).unwrap());
            t.with_timezone(&tz).format("%H:%M %d/%m/%Y").to_string()
        })
        .unwrap_or_default()
}

/// All anchors, any timeline, newest first (excludes the live wc commit).
fn all_anchors(fp: &Fp) -> Result<Vec<(Commit, AnchorMeta)>> {
    let store = fp.repo.store();
    let mut seen = std::collections::HashSet::new();
    let mut queue: Vec<_> = fp.repo.view().heads().iter().cloned().collect();
    let mut anchors = Vec::new();
    while let Some(id) = queue.pop() {
        if !seen.insert(id.clone()) || id == *store.root_commit_id() {
            continue;
        }
        let commit = store.get_commit(&id)?;
        queue.extend(commit.parent_ids().iter().cloned());
        if let Some(meta) = AnchorMeta::parse(commit.description()) {
            anchors.push((commit, meta));
        }
    }
    anchors.sort_by_key(|(c, _)| std::cmp::Reverse(c.committer().timestamp.timestamp.0));
    Ok(anchors)
}

#[derive(Serialize)]
struct StageCkpt {
    id: String,
    time: String,
    subject: String,
    session: String,
    agent: String,
    phase: String,
}

#[derive(Serialize)]
struct Stage {
    base: String,
    current: bool,
    count: usize,
    latest: String,
    ckpts: Vec<StageCkpt>,
    commit: Option<()>, // commit details not populated in v1; UIs tolerate null
}

#[derive(Serialize)]
struct Stages {
    head: String,
    stages: Vec<Stage>,
}

/// `fp _stages`: anchors bucketed by the git HEAD they were sealed on.
pub fn stages(fp: &Fp) -> Result<()> {
    let head = git_head(fp.workspace.workspace_root()).unwrap_or_default();
    let mut order: Vec<String> = Vec::new();
    let mut buckets: std::collections::HashMap<String, Vec<StageCkpt>> = Default::default();
    for (commit, meta) in all_anchors(fp)? {
        let base = meta.base.clone().unwrap_or_default();
        if !buckets.contains_key(&base) {
            order.push(base.clone());
        }
        buckets.entry(base).or_default().push(StageCkpt {
            id: short_id(&commit),
            time: time_of(&commit),
            subject: meta.title.clone(),
            session: meta.session.clone(),
            agent: meta.speedster.clone(),
            phase: meta.phase.clone(),
        });
    }
    // The current stage is always surfaced, even when empty.
    if !order.contains(&head) {
        order.insert(0, head.clone());
        buckets.entry(head.clone()).or_default();
    }
    let stages = order
        .into_iter()
        .map(|base| {
            let ckpts = buckets.remove(&base).unwrap_or_default();
            Stage {
                current: base == head,
                count: ckpts.len(),
                latest: ckpts.first().map(|c| c.time.clone()).unwrap_or_default(),
                ckpts,
                base,
                commit: None,
            }
        })
        .collect();
    println!("{}", serde_json::to_string(&Stages { head, stages })?);
    Ok(())
}

/// `fp _active`: change ids on the current path, one per line.
pub fn active(fp: &Fp) -> Result<()> {
    let wc = fp.wc_commit()?;
    let store = fp.repo.store();
    let mut seen = std::collections::HashSet::new();
    let mut queue = vec![wc.id().clone()];
    while let Some(id) = queue.pop() {
        if !seen.insert(id.clone()) || id == *store.root_commit_id() {
            continue;
        }
        let commit = store.get_commit(&id)?;
        println!("{}", short_id(&commit));
        queue.extend(commit.parent_ids().iter().cloned());
    }
    Ok(())
}

/// `fp _files <id>`: `status<TAB>path` per changed file (anchor vs parent).
pub fn files(fp: &Fp, id: &str) -> Result<()> {
    let commit = crate::ids::resolve(fp, id)?;
    let parent = fp
        .repo
        .store()
        .get_commit(commit.parent_ids().first().context("no parent")?)?;
    for entry in crate::diff::entries(&parent.tree(), &commit.tree())? {
        println!("{}\t{}", entry.status, entry.path);
    }
    Ok(())
}

/// `fp _parent <id>`: the parent anchor's change id (empty at root).
pub fn parent(fp: &Fp, id: &str) -> Result<()> {
    let commit = crate::ids::resolve(fp, id)?;
    if let Some(pid) = commit.parent_ids().first()
        && *pid != *fp.repo.store().root_commit_id()
    {
        println!("{}", short_id(&fp.repo.store().get_commit(pid)?));
    }
    Ok(())
}

/// `fp _show <rev> <path>`: raw file content at a revision. `<id>-` means
/// the id's parent (jj revset convention kept for UI compatibility).
pub fn show(fp: &Fp, rev: &str, file: &str) -> Result<()> {
    let (id, want_parent) = match rev.strip_suffix('-') {
        Some(base) => (base, true),
        None => (rev, false),
    };
    let mut commit = crate::ids::resolve(fp, id)?;
    if want_parent {
        let pid = commit.parent_ids().first().context("no parent")?.clone();
        commit = fp.repo.store().get_commit(&pid)?;
    }
    let path = RepoPath::from_internal_string(file)
        .with_context(|| format!("bad path: {file}"))?;
    let value = block_on(commit.tree().path_value(path))?;
    // Absent at this rev (e.g. an added file's parent side) → empty content.
    // Conflicted values are not expected in fp stores; treated as absent.
    let Some(Some(TreeValue::File { id: file_id, .. })) = value.as_resolved() else {
        return Ok(());
    };
    let file_id = file_id.clone();
    let mut reader = block_on(fp.repo.store().read_file(path, &file_id))?;
    let mut bytes = Vec::new();
    block_on(reader.read_to_end(&mut bytes))?;
    use std::io::Write as _;
    std::io::stdout().write_all(&bytes)?;
    Ok(())
}

/// `fp _tip`: operation-log head fingerprint for cheap change polling.
pub fn tip(fp: &Fp) -> Result<()> {
    let hex = fp.repo.op_id().hex();
    println!("{}", &hex[..16.min(hex.len())]);
    Ok(())
}

/// `fp status`: human-readable summary (used by the extension's status doc).
pub fn status(fp: &mut Fp) -> Result<()> {
    let wc = fp.wc_commit()?;
    let tree = crate::diff::workspace_tree(fp)?;
    let entries = crate::diff::entries(&wc.tree(), &tree)?;
    if entries.is_empty() {
        println!("workspace clean (matches the last anchor)");
    } else {
        println!("un-anchored changes:");
        for e in &entries {
            println!("  {} {}", e.status, e.path);
        }
    }
    crate::log::print_log(fp)?;
    Ok(())
}
