//! Safe-zone exclusions: paths matching `.fp/safe` globs (default `.env*`)
//! are never tracked by anchors — secrets never enter the store — and never
//! rewritten by timetravel.

use std::path::Path;

use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use jj_lib::fileset::{FilePattern, FilesetExpression};
use jj_lib::matchers::Matcher;
use jj_lib::repo_path::RepoPathBuf;

pub struct SafeZone {
    /// For matching workspace-relative path strings (checkout preservation).
    pub globset: GlobSet,
    /// For excluding paths from snapshot tracking.
    pub matcher: Box<dyn Matcher>,
}

const DEFAULT_PATTERNS: &str = ".env\n.env.*\n";

pub fn load(workspace_root: &Path) -> Result<SafeZone> {
    let list = workspace_root.join(".fp/safe");
    let patterns = std::fs::read_to_string(list).unwrap_or_else(|_| DEFAULT_PATTERNS.to_string());

    let mut globset = GlobSetBuilder::new();
    // flashpoint's own state dir is never part of history.
    let mut exprs = vec![FilesetExpression::pattern(FilePattern::PrefixPath(
        RepoPathBuf::from_internal_string(".fp").expect("valid path"),
    ))];
    for line in patterns.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let glob = Glob::new(line).with_context(|| format!("bad glob in .fp/safe: {line}"))?;
        globset.add(glob.clone());
        exprs.push(FilesetExpression::pattern(FilePattern::FileGlob {
            dir: RepoPathBuf::root(),
            pattern: Box::new(glob),
        }));
    }
    Ok(SafeZone {
        globset: globset.build()?,
        matcher: FilesetExpression::union_all(exprs).to_matcher(),
    })
}
