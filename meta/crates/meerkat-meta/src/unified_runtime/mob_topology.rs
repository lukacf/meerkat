//! Where the topology state of a mob a host serves beside its default mob
//! is kept, and what its removal after that mob's destroy selects
//! ([`super::mob_entry::MobResolver::release_topology_state_of_a_destroyed_mob`]).
//!
//! The default mob's state file is where its launch said. Any other mob's
//! is beside that mob's store file and named after it, with the mob's
//! incarnation in the name, so a mob created under the id of a destroyed
//! mob never opens the destroyed mob's file, also when the removal below
//! did not run. The file's own `authority` names the mob, and a file that
//! names another mob is refused where it is opened
//! ([`crate::topology_control::HostTopologyControl::controller_of`]).

use std::path::{Path, PathBuf};

use meerkat_mob::MobHandle;

/// What the name of a topology state file of a mob beside the default mob
/// ends with. Its lock and a write under way carry one more extension.
const STATE_FILE_SUFFIX: &str = "topology-control.json";

/// The topology state file of the mob `handle` serves, a mob `set` built
/// for the host beside its default mob: beside the mob's store file, named
/// after it and after the mob's incarnation. `None` for a mob the set keeps
/// in memory: it has no store file, and no durable place for operator
/// intent.
pub(super) fn state_path_of(
    set: &meerkat_mob_mcp::MobMcpState,
    handle: &MobHandle,
) -> Option<PathBuf> {
    let store = set.persistent_mob_store_path(handle.mob_id())?;
    let mut name = store.file_name()?.to_os_string();
    name.push(format!(".{}.{STATE_FILE_SUFFIX}", handle.incarnation()));
    Some(store.with_file_name(name))
}

/// Whether the file named `name` is topology state of the mob whose store
/// file is named `store_name`, of any incarnation: a state file, its lock,
/// or a write that did not end. A mob id has no `.` in a store file's name,
/// so no other mob's store name starts with this one and a `.`.
fn is_topology_state_of(store_name: &str, name: &str) -> bool {
    name.strip_prefix(store_name)
        .is_some_and(|rest| rest.starts_with('.') && rest.contains(STATE_FILE_SUFFIX))
}

/// Remove every topology state file kept beside the store file `store`: of
/// each incarnation of that store's mob, with its lock. A file that is
/// already gone is no failure.
pub(super) fn remove_state_files_beside(store: &Path) -> std::io::Result<()> {
    let (Some(dir), Some(store_name)) = (
        store.parent(),
        store.file_name().and_then(|name| name.to_str()),
    ) else {
        return Ok(());
    };
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        let ours = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| is_topology_state_of(store_name, name));
        if !ours {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{STATE_FILE_SUFFIX, is_topology_state_of};

    /// The removal selects the state files of one store's mob, of every
    /// incarnation and with their locks, and nothing of a mob whose id
    /// only starts with that mob's id, of a file whose name only starts
    /// with the store's name, the store itself or its manifest.
    #[test]
    fn the_removal_selects_the_state_files_of_one_mob_alone() {
        let store = "mob--crew.db";
        for ours in [
            format!("{store}.0f0e0d0c-0b0a-4908-8706-050403020100.{STATE_FILE_SUFFIX}"),
            format!("{store}.0f0e0d0c-0b0a-4908-8706-050403020100.{STATE_FILE_SUFFIX}.lock"),
            format!("{store}.0f0e0d0c-0b0a-4908-8706-050403020100.{STATE_FILE_SUFFIX}.tmp"),
        ] {
            assert!(is_topology_state_of(store, &ours), "{ours}");
        }
        for theirs in [
            store.to_string(),
            format!("{store}.composition.json"),
            format!("{store}-wal"),
            format!("mob--crew-two.db.0f0e0d0c-0b0a-4908-8706-050403020100.{STATE_FILE_SUFFIX}"),
            format!("{store}x.0f0e0d0c-0b0a-4908-8706-050403020100.{STATE_FILE_SUFFIX}"),
            format!("{store}{STATE_FILE_SUFFIX}"),
            format!("mob--crew%2Edb.x.{STATE_FILE_SUFFIX}"),
            STATE_FILE_SUFFIX.to_string(),
        ] {
            assert!(!is_topology_state_of(store, &theirs), "{theirs}");
        }
    }
}
