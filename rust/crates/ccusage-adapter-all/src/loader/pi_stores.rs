use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use ccusage_adapter_common::source::SourceOptions;

use super::ResolvedNamedPiStore;
use crate::{Result, adapter::pi, cli::NamedPiStore};

pub(super) fn resolve_named_pi_store_paths(
    stores: &[NamedPiStore],
    default_pi: SourceOptions<'_>,
) -> Result<Vec<ResolvedNamedPiStore>> {
    let mut owners = BTreeMap::<PathBuf, String>::new();
    for path in pi::default_paths(default_pi.pi_path, default_pi.pi_agent_dir)? {
        owners.insert(path_key(&path), "default pi store".to_string());
    }

    let mut resolved = Vec::with_capacity(stores.len());
    for store in stores {
        let mut paths = Vec::new();
        let mut collision_owners = BTreeSet::new();
        for path in pi::named_store_paths(&store.path)? {
            let key = path_key(&path);
            if let Some(owner) = owners
                .iter()
                .find(|(owned, _)| paths_overlap(&key, owned))
                .map(|(_, owner)| owner.clone())
            {
                collision_owners.insert(owner);
                continue;
            }
            owners.insert(key, format!("pi.stores name '{}'", store.name));
            paths.push(path);
        }

        if !collision_owners.is_empty() {
            return Err(crate::cli_error(format!(
                "Invalid ccusage config: pi.stores name '{}' paths overlap {}",
                store.name,
                collision_owners.into_iter().collect::<Vec<_>>().join(", ")
            )));
        }

        resolved.push(ResolvedNamedPiStore {
            name: store.name.clone(),
            paths,
        });
    }
    Ok(resolved)
}

fn path_key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}
