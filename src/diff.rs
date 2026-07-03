//! Tree diffing: what changed at an anchor, since an anchor, or between two.

use anyhow::Result;
use futures::StreamExt as _;
use jj_lib::gitignore::GitIgnoreFile;
use jj_lib::matchers::{DifferenceMatcher, EverythingMatcher, NothingMatcher};
use jj_lib::merged_tree::MergedTree;
use jj_lib::working_copy::SnapshotOptions;
use pollster::block_on;

use crate::repo::Fp;

pub struct Entry {
    pub status: char, // 'A' added, 'M' modified, 'D' deleted
    pub path: String,
}

pub fn entries(before: &MergedTree, after: &MergedTree) -> Result<Vec<Entry>> {
    block_on(async {
        let mut out = Vec::new();
        let mut stream = before.diff_stream(after, &EverythingMatcher);
        while let Some(entry) = stream.next().await {
            let values = entry.values?;
            let status = match (values.before.is_present(), values.after.is_present()) {
                (false, true) => 'A',
                (true, false) => 'D',
                _ => 'M',
            };
            out.push(Entry {
                status,
                path: entry.path.as_internal_file_string().to_string(),
            });
        }
        Ok(out)
    })
}

/// Snapshot the current workspace files into a tree WITHOUT sealing anything.
/// The lock is dropped unfinished, so no repo/working-copy state changes.
pub fn workspace_tree(fp: &mut Fp) -> Result<MergedTree> {
    let safe = crate::safezone::load(fp.workspace.workspace_root())?;
    let start_tracking = DifferenceMatcher::new(EverythingMatcher, safe.matcher);
    let mut locked_ws = block_on(fp.workspace.start_working_copy_mutation())?;
    let options = SnapshotOptions {
        base_ignores: GitIgnoreFile::empty(),
        progress: None,
        start_tracking_matcher: &start_tracking,
        force_tracking_matcher: &NothingMatcher,
        max_new_file_size: 8 * 1024 * 1024,
    };
    let (tree, _stats) = block_on(locked_ws.locked_wc().snapshot(&options))?;
    drop(locked_ws);
    Ok(tree)
}

pub fn print(entries: &[Entry]) {
    for e in entries {
        println!("{} {}", e.status, e.path);
    }
    let n = entries.len();
    println!("{n} file{} changed", if n == 1 { "" } else { "s" });
}
