//! Anchor metadata: the commit-description format (title + Fp-* trailers)
//! and git HEAD detection for the Fp-Base trailer.

use std::fmt::Write as _;
use std::path::Path;

/// Marker every anchor description contains; anchor discovery keys off it.
pub const SESSION_TRAILER: &str = "Fp-Session:";

pub struct AnchorMeta {
    pub title: String,
    pub session: String,
    pub speedster: String,
    pub phase: String,
    pub base: Option<String>,
}

impl AnchorMeta {
    pub fn to_description(&self) -> String {
        let mut d = format!(
            "{}\n\nFp-Session: {}\nFp-Speedster: {}\nFp-Phase: {}\n",
            self.title, self.session, self.speedster, self.phase
        );
        if let Some(base) = &self.base {
            let _ = writeln!(d, "Fp-Base: {base}");
        }
        d
    }

    pub fn parse(description: &str) -> Option<Self> {
        if !description.contains(SESSION_TRAILER) {
            return None;
        }
        let title = description.lines().next().unwrap_or("").to_string();
        let get = |key: &str| trailer(description, key);
        Some(Self {
            title,
            session: get("Fp-Session")?,
            speedster: get("Fp-Speedster").unwrap_or_default(),
            phase: get("Fp-Phase").unwrap_or_default(),
            base: get("Fp-Base"),
        })
    }
}

fn trailer(description: &str, key: &str) -> Option<String> {
    description.lines().find_map(|line| {
        line.strip_prefix(key)
            .and_then(|rest| rest.strip_prefix(':'))
            .map(|v| v.trim().to_string())
    })
}

/// Is this description an anchor (vs the live wc commit or foreign commits)?
pub fn is_anchor(description: &str) -> bool {
    description.contains(SESSION_TRAILER)
}

/// Resolve the real git repo's HEAD commit sha for `root`, if it is a git
/// repo. Read-only: never writes to the user's `.git`.
pub fn git_head(root: &Path) -> Option<String> {
    let git = root.join(".git");
    let head = std::fs::read_to_string(git.join("HEAD")).ok()?;
    let head = head.trim();
    let Some(refname) = head.strip_prefix("ref: ") else {
        return Some(head.to_string()); // detached HEAD
    };
    if let Ok(sha) = std::fs::read_to_string(git.join(refname)) {
        return Some(sha.trim().to_string());
    }
    // Ref may be packed.
    let packed = std::fs::read_to_string(git.join("packed-refs")).ok()?;
    packed.lines().find_map(|line| {
        let (sha, name) = line.split_once(' ')?;
        (name == refname).then(|| sha.to_string())
    })
}

pub fn now_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}
