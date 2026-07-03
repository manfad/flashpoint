mod diff;
mod hook;
mod ids;
mod log;
mod meta;
mod repo;
mod safezone;
mod timeline;
mod timetravel;

use anyhow::Result;
use clap::{Parser, Subcommand};
use jj_lib::repo::Repo as _;

use crate::meta::{now_stamp, AnchorMeta};
use crate::repo::Fp;

/// flashpoint — checkpoints for AI-agent coding sessions.
#[derive(Parser)]
#[command(name = "fp", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Seal the current working copy as a safe point.
    Anchor {
        /// Title for the anchor (defaults to a timestamp).
        #[arg(short, long)]
        title: Option<String>,
        /// Which speedster (agent) triggered this anchor.
        #[arg(long, default_value = "human", env = "FP_SPEEDSTER")]
        speedster: String,
        /// Session id grouping this anchor.
        #[arg(long, default_value = "manual", env = "FP_SESSION")]
        session: String,
        /// Hook phase: pre, post, or manual.
        #[arg(long, default_value = "manual", env = "FP_PHASE")]
        phase: String,
    },
    /// List anchors on the current timeline.
    Log,
    /// What changed at / since an anchor.
    ///
    /// `fp diff <id>`: what that anchor's turn changed (anchor vs parent).
    /// `fp diff <id> --workspace`: what timetraveling there would rewrite.
    /// `fp diff <a> <b>`: between two anchors.
    Diff {
        id: String,
        /// Second anchor to compare against.
        to: Option<String>,
        /// Compare the anchor against the current workspace instead.
        #[arg(short, long, conflicts_with = "to")]
        workspace: bool,
    },
    /// Entry point for speedster hook configs (reads payload JSON on stdin).
    Hook {
        /// Which speedster's hook fired.
        #[arg(long, default_value = "claude-code")]
        speedster: String,
        /// pre (turn about to start) or post (turn ended).
        #[arg(long)]
        phase: String,
    },
    /// Machine-readable timeline JSON for UI surfaces (porcelain).
    #[command(name = "_timeline", hide = true)]
    Timeline,
    /// Put files back exactly as they were at an anchor. Nothing forks
    /// until you change something afterwards.
    Timetravel {
        id: String,
        /// Skip the confirmation prompt.
        #[arg(short, long)]
        yes: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;

    match cli.command {
        Command::Anchor {
            title,
            speedster,
            session,
            phase,
        } => {
            let mut fp = Fp::open_or_init(&cwd)?;
            let meta = AnchorMeta {
                title: title.unwrap_or_else(now_stamp),
                session,
                speedster,
                phase,
                base: meta::git_head(fp.workspace.workspace_root()),
            };
            match fp.seal_anchor(&meta.to_description())? {
                Some(sealed) => println!("anchored: {}", log::short_id(&sealed)),
                None => println!("nothing changed since the last anchor"),
            }
        }
        Command::Log => {
            let fp = Fp::open(&cwd)?;
            log::print_log(&fp)?;
        }
        Command::Diff { id, to, workspace } => {
            let mut fp = Fp::open(&cwd)?;
            let anchor = ids::resolve(&fp, &id)?;
            let entries = if workspace {
                // What restoring this anchor would rewrite.
                let wc_tree = diff::workspace_tree(&mut fp)?;
                diff::entries(&wc_tree, &anchor.tree())?
            } else if let Some(to) = to {
                let other = ids::resolve(&fp, &to)?;
                diff::entries(&anchor.tree(), &other.tree())?
            } else {
                // What this anchor's turn changed.
                let parent = fp
                    .repo
                    .store()
                    .get_commit(anchor.parent_ids().first().expect("commit has a parent"))?;
                diff::entries(&parent.tree(), &anchor.tree())?
            };
            diff::print(&entries);
        }
        Command::Hook { speedster, phase } => {
            hook::run(&speedster, &phase)?;
        }
        Command::Timeline => {
            let fp = Fp::open(&cwd)?;
            timeline::print(&fp)?;
        }
        Command::Timetravel { id, yes } => {
            let mut fp = Fp::open(&cwd)?;
            timetravel::run(&mut fp, &id, yes)?;
        }
    }
    Ok(())
}
