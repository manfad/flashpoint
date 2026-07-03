//! Opening/initializing the flashpoint store (a jj-format repo in `.jj/`)
//! and the core seal-anchor operation.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use jj_lib::commit::Commit;
use jj_lib::config::{ConfigLayer, ConfigSource, StackedConfig};
use jj_lib::gitignore::GitIgnoreFile;
use jj_lib::matchers::{DifferenceMatcher, EverythingMatcher, NothingMatcher};
use jj_lib::ref_name::WorkspaceNameBuf;
use jj_lib::repo::{ReadonlyRepo, Repo, StoreFactories};
use jj_lib::settings::UserSettings;
use jj_lib::working_copy::SnapshotOptions;
use jj_lib::workspace::{default_working_copy_factories, Workspace};
use pollster::block_on;

pub struct Fp {
    pub workspace: Workspace,
    pub repo: Arc<ReadonlyRepo>,
    pub workspace_name: WorkspaceNameBuf,
}

fn settings() -> Result<UserSettings> {
    let mut config = StackedConfig::with_defaults();
    config.add_layer(ConfigLayer::parse(
        ConfigSource::User,
        r#"
user.name = "flashpoint"
user.email = "flashpoint@localhost"
"#,
    )?);
    Ok(UserSettings::from_config(config)?)
}

/// Keep the store invisible to the user's real git repo: add `.jj/` and
/// `.fp/` to `.git/info/exclude` (never touches tracked files). Best-effort.
fn exclude_from_git(root: &Path) {
    let info = root.join(".git/info");
    if !root.join(".git").exists() {
        return;
    }
    let exclude = info.join("exclude");
    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    let missing: Vec<&str> = [".jj/", ".fp/"]
        .into_iter()
        .filter(|line| !existing.lines().any(|l| l.trim() == *line))
        .collect();
    if missing.is_empty() {
        return;
    }
    let _ = std::fs::create_dir_all(&info);
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    for line in missing {
        content.push_str(line);
        content.push('\n');
    }
    let _ = std::fs::write(&exclude, content);
}

/// Find the workspace root: the nearest ancestor of `start` containing `.jj`.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|p| p.join(".jj").exists())
        .map(Path::to_path_buf)
}

impl Fp {
    /// Open the store for `cwd`, initializing one at `cwd` if none exists
    /// anywhere above it.
    pub fn open_or_init(cwd: &Path) -> Result<Self> {
        let settings = settings()?;
        let workspace = match find_root(cwd) {
            Some(root) => Workspace::load(
                &settings,
                &root,
                &StoreFactories::default(),
                &default_working_copy_factories(),
            )?,
            None => {
                let workspace = block_on(Workspace::init_internal_git(&settings, cwd))?.0;
                exclude_from_git(cwd);
                workspace
            }
        };
        let repo = block_on(workspace.repo_loader().load_at_head())?;
        let workspace_name = workspace.workspace_name().to_owned();
        Ok(Self {
            workspace,
            repo,
            workspace_name,
        })
    }

    /// Open the store for `cwd`; error if none exists.
    pub fn open(cwd: &Path) -> Result<Self> {
        let settings = settings()?;
        let root = find_root(cwd)
            .context("no flashpoint store here — run `fp anchor` first to create one")?;
        let workspace = Workspace::load(
            &settings,
            &root,
            &StoreFactories::default(),
            &default_working_copy_factories(),
        )?;
        let repo = block_on(workspace.repo_loader().load_at_head())?;
        let workspace_name = workspace.workspace_name().to_owned();
        Ok(Self {
            workspace,
            repo,
            workspace_name,
        })
    }

    pub fn wc_commit(&self) -> Result<Commit> {
        let id = self
            .repo
            .view()
            .get_wc_commit_id(&self.workspace_name)
            .context("workspace has no working-copy commit")?;
        Ok(self.repo.store().get_commit(id)?)
    }

    /// Seal the current working copy as an anchor. Returns the sealed commit,
    /// or `None` when nothing changed since the last anchor (diff-gated).
    pub fn seal_anchor(&mut self, description: &str) -> Result<Option<Commit>> {
        let wc_commit = self.wc_commit()?;

        // Safe-zone paths are never started tracking: secrets stay out of
        // the store entirely.
        let safe = crate::safezone::load(self.workspace.workspace_root())?;
        let start_tracking = DifferenceMatcher::new(EverythingMatcher, safe.matcher);

        let mut locked_ws = block_on(self.workspace.start_working_copy_mutation())?;
        let options = SnapshotOptions {
            base_ignores: GitIgnoreFile::empty(),
            progress: None,
            start_tracking_matcher: &start_tracking,
            force_tracking_matcher: &NothingMatcher,
            max_new_file_size: 8 * 1024 * 1024,
        };
        let (new_tree, _stats) = block_on(locked_ws.locked_wc().snapshot(&options))?;

        if new_tree.tree_ids_and_labels() == wc_commit.tree().tree_ids_and_labels() {
            block_on(locked_ws.finish(self.repo.op_id().clone()))?;
            return Ok(None);
        }

        // `jj commit` flow: seal the wc commit with the snapshot tree and the
        // anchor description, then open a fresh empty wc commit on top.
        let mut tx = self.repo.start_transaction();
        tx.set_is_snapshot(true);
        tx.set_workspace_name(&self.workspace_name);
        let sealed = block_on(
            tx.repo_mut()
                .rewrite_commit(&wc_commit)
                .set_tree(new_tree.clone())
                .set_description(description)
                .write(),
        )?;
        let next_wc = block_on(
            tx.repo_mut()
                .new_commit(vec![sealed.id().clone()], new_tree)
                .write(),
        )?;
        block_on(tx.repo_mut().edit(self.workspace_name.clone(), &next_wc))?;
        block_on(tx.repo_mut().rebase_descendants())?;
        self.repo = block_on(tx.commit("anchor"))?;

        block_on(locked_ws.locked_wc().reset(&next_wc))?;
        block_on(locked_ws.finish(self.repo.op_id().clone()))?;
        Ok(Some(sealed))
    }
}
