//! Listing anchors. An anchor is any ancestor of the working-copy commit
//! with a non-empty description (the current empty wc commit is not one).

use anyhow::Result;
use jj_lib::backend::CommitId;
use jj_lib::commit::Commit;
use jj_lib::hex_util::encode_reverse_hex;
use jj_lib::object_id::ObjectId;
use jj_lib::repo::Repo;

use crate::repo::Fp;

/// The user-facing anchor id: a short prefix of the change id, shown in jj's
/// reverse-hex alphabet (z..k) so it never collides with commit-id hex.
pub fn short_id(commit: &Commit) -> String {
    let mut id = encode_reverse_hex(commit.change_id().to_bytes().as_slice());
    id.truncate(8);
    id
}

fn timestamp(commit: &Commit) -> String {
    let ts = &commit.committer().timestamp;
    chrono::DateTime::from_timestamp_millis(ts.timestamp.0)
        .map(|t| {
            let tz = chrono::FixedOffset::east_opt(ts.tz_offset * 60)
                .unwrap_or_else(|| chrono::FixedOffset::east_opt(0).unwrap());
            t.with_timezone(&tz).format("%Y-%m-%d %H:%M:%S").to_string()
        })
        .unwrap_or_default()
}

/// Anchors from newest to oldest, walking first parents from `from`.
pub fn anchors_from(fp: &Fp, from: &CommitId) -> Result<Vec<Commit>> {
    let store = fp.repo.store();
    let mut anchors = Vec::new();
    let mut next = Some(from.clone());
    while let Some(id) = next {
        if id == *fp.repo.store().root_commit_id() {
            break;
        }
        let commit = store.get_commit(&id)?;
        next = commit.parent_ids().first().cloned();
        if crate::meta::is_anchor(commit.description()) {
            anchors.push(commit);
        }
    }
    Ok(anchors)
}

pub fn print_log(fp: &Fp) -> Result<()> {
    let wc = fp.wc_commit()?;
    let anchors = anchors_from(fp, wc.id())?;
    if anchors.is_empty() {
        println!("no anchors yet — run `fp anchor`");
        return Ok(());
    }
    for commit in &anchors {
        let title = commit.description().lines().next().unwrap_or("");
        println!("{}  {}  {}", short_id(commit), timestamp(commit), title);
    }
    Ok(())
}
