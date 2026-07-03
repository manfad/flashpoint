//! `fp _timeline`: the JSON view-model UIs render (ported verbatim from
//! jjckpt's porcelain contract: `{ rows: [...], laneCount: 1|2 }`).

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use jj_lib::commit::Commit;
use jj_lib::repo::Repo;
use serde::Serialize;

use crate::log::short_id;
use crate::meta::AnchorMeta;
use crate::repo::Fp;

#[derive(Serialize)]
struct Row {
    id: String,
    parents: Vec<String>,
    time: String,
    current: bool,
    empty: bool,
    subject: String,
    session: String,
    agent: String,
    phase: String,
    agents: Vec<String>,
    index: usize,
    lane: u8,
    #[serde(rename = "onCurrentPath")]
    on_current_path: bool,
    label: String,
    description: String,
    author: String,
    timestamp: String,
    details: String,
}

#[derive(Serialize)]
struct Model {
    rows: Vec<Row>,
    #[serde(rename = "laneCount")]
    lane_count: u8,
}

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

fn pretty_agent(slug: &str) -> String {
    if slug.is_empty() {
        return String::new();
    }
    slug.split(['-', '_'])
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A title that is just a timestamp means "no summary was available".
fn is_timestamp_title(title: &str) -> bool {
    title.len() == 19
        && title.as_bytes()[4] == b'-'
        && title.as_bytes()[13] == b':'
        && title.chars().all(|c| c.is_ascii_digit() || "-: ".contains(c))
}

struct Node {
    commit: Commit,
    meta: Option<AnchorMeta>,
    current: bool,
}

/// All visible anchors (every timeline) plus the working-copy commit,
/// newest first.
fn collect_nodes(fp: &Fp) -> Result<Vec<Node>> {
    let wc = fp.wc_commit()?;
    let store = fp.repo.store();
    let mut seen: HashSet<jj_lib::backend::CommitId> = HashSet::new();
    let mut queue: Vec<jj_lib::backend::CommitId> = fp.repo.view().heads().iter().cloned().collect();
    let mut nodes = Vec::new();
    while let Some(id) = queue.pop() {
        if !seen.insert(id.clone()) || id == *store.root_commit_id() {
            continue;
        }
        let commit = store.get_commit(&id)?;
        queue.extend(commit.parent_ids().iter().cloned());
        let meta = AnchorMeta::parse(commit.description());
        let current = commit.id() == wc.id();
        if meta.is_some() || current {
            nodes.push(Node {
                commit,
                meta,
                current,
            });
        }
    }
    // jj log order approximated: newest committer timestamp first, working
    // copy pinned to the top.
    nodes.sort_by_key(|n| {
        (
            !n.current,
            std::cmp::Reverse(n.commit.committer().timestamp.timestamp.0),
        )
    });
    Ok(nodes)
}

/// Change ids on the current path (ancestors of the working copy).
fn active_ids(fp: &Fp) -> Result<HashSet<String>> {
    let wc = fp.wc_commit()?;
    let store = fp.repo.store();
    let mut active = HashSet::new();
    let mut queue = vec![wc.id().clone()];
    let mut seen = HashSet::new();
    while let Some(id) = queue.pop() {
        if !seen.insert(id.clone()) || id == *store.root_commit_id() {
            continue;
        }
        let commit = store.get_commit(&id)?;
        active.insert(short_id(&commit));
        queue.extend(commit.parent_ids().iter().cloned());
    }
    Ok(active)
}

pub fn print(fp: &Fp) -> Result<()> {
    let nodes = collect_nodes(fp)?;
    let active = active_ids(fp)?;
    let present: HashSet<String> = nodes.iter().map(|n| short_id(&n.commit)).collect();

    // Per-speedster checkpoint numbering, oldest = #1.
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut numbers: HashMap<String, usize> = HashMap::new();
    for node in nodes.iter().rev() {
        if let Some(meta) = &node.meta {
            let n = counts.entry(meta.speedster.clone()).or_insert(0);
            *n += 1;
            numbers.insert(short_id(&node.commit), *n);
        }
    }

    let mut rows = Vec::new();
    let mut lane_count = 1;
    for (index, node) in nodes.iter().enumerate() {
        let id = short_id(&node.commit);
        let on_current_path = node.current || active.contains(&id);
        let lane = u8::from(!on_current_path);
        if lane == 1 {
            lane_count = 2;
        }
        let time = time_of(&node.commit);
        let empty = node
            .commit
            .parent_ids()
            .first()
            .and_then(|p| fp.repo.store().get_commit(p).ok())
            .is_some_and(|p| {
                p.tree().tree_ids_and_labels() == node.commit.tree().tree_ids_and_labels()
            });

        let (subject, session, agent, phase) = match &node.meta {
            Some(m) => (
                m.title.clone(),
                m.session.clone(),
                m.speedster.clone(),
                m.phase.clone(),
            ),
            None => Default::default(),
        };
        let pretty = pretty_agent(&agent);
        let kind = if matches!(agent.as_str(), "human" | "vscode" | "checkpoint") {
            "Safepoint"
        } else {
            "Checkpoint"
        };
        let number = numbers.get(&id).copied().unwrap_or(0);

        let (label, description, author) = if node.current && node.meta.is_none() {
            (
                "Working copy".to_string(),
                "current".to_string(),
                "Working copy".to_string(),
            )
        } else if !subject.is_empty() && !is_timestamp_title(&subject) {
            (
                subject.clone(),
                format!("{kind} #{number} · {pretty}"),
                pretty.clone(),
            )
        } else {
            (
                format!("{kind} {number} — {pretty}"),
                time.clone(),
                pretty.clone(),
            )
        };

        let details = [
            id.clone(),
            time.clone(),
            if node.current {
                "working copy".to_string()
            } else {
                String::new()
            },
            if on_current_path {
                String::new()
            } else {
                "off current path".to_string()
            },
            if node.current && node.meta.is_none() {
                String::new()
            } else {
                pretty.clone()
            },
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");

        rows.push(Row {
            parents: node
                .commit
                .parent_ids()
                .iter()
                .filter_map(|p| fp.repo.store().get_commit(p).ok())
                .map(|p| short_id(&p))
                .filter(|p| present.contains(p))
                .collect(),
            id,
            timestamp: time.clone(),
            time,
            current: node.current,
            empty,
            subject,
            session,
            agent: agent.clone(),
            phase,
            agents: if agent.is_empty() { vec![] } else { vec![agent] },
            index,
            lane,
            on_current_path,
            label,
            description,
            author,
            details,
        });
    }

    println!(
        "{}",
        serde_json::to_string(&Model {
            rows,
            lane_count,
        })?
    );
    Ok(())
}
