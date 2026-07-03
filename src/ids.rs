//! Resolving user-facing anchor ids (short change-id prefixes in jj's
//! reverse-hex alphabet) to commits.

use anyhow::{bail, Context, Result};
use jj_lib::commit::Commit;
use jj_lib::object_id::{HexPrefix, PrefixResolution};
use jj_lib::repo::Repo;

use crate::repo::Fp;

pub fn resolve(fp: &Fp, id: &str) -> Result<Commit> {
    let prefix = HexPrefix::try_from_reverse_hex(id)
        .with_context(|| format!("'{id}' is not a valid anchor id"))?;
    match fp.repo.resolve_change_id_prefix(&prefix)? {
        PrefixResolution::SingleMatch(targets) => {
            let commit_id = targets
                .visible_with_offsets()
                .map(|(_, id)| id.clone())
                .next()
                .or_else(|| targets.targets.first().map(|(id, _)| id.clone()))
                .with_context(|| format!("anchor '{id}' has no commits"))?;
            Ok(fp.repo.store().get_commit(&commit_id)?)
        }
        PrefixResolution::NoMatch => bail!("no anchor matches '{id}' — see `fp log`"),
        PrefixResolution::AmbiguousMatch => {
            bail!("anchor id '{id}' is ambiguous — give more characters")
        }
    }
}
