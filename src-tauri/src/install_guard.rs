//! Remembers how this copy of Kadr was installed, and refuses to let an update
//! delete a folder that holds the user's own files.
//!
//! The update path on Windows is:
//!
//! 1. replace the exe in place (no installer) when the folder is writable;
//! 2. otherwise run the installer, and the installer *uninstalls the previous
//!    version first*.
//!
//! Step 2 is where the danger is. The uninstaller removes the install
//! directory (`$INSTDIR`) plus the runtime data folder. That is correct for a
//! normal install into `%LOCALAPPDATA%\\Programs\\kadr`. It is catastrophic
//! for anyone who unpacked the portable build into a folder that also holds
//! their own files, because "uninstall the previous version" then means
//! "delete the folder the user put their things in".
//!
//! Two answers are written down here:
//!
//! * where this copy lives, recorded at first run, so the installer path can be
//!   refused when the folder is not a plain install directory;
//! * that a data folder kept beside the exe belongs to the user, so it is never
//!   removed by an uninstaller.

use std::path::{Path, PathBuf};

use crate::config::KensakuConfig;

/// Where the "this copy is portable" marker may sit, beside the executable.
const PORTABLE_MARKERS: [&str; 2] = ["portable", "kensaku.portable"];

fn record_path() -> PathBuf {
    KensakuConfig::config_dir().join("install.json")
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InstallRecord {
    /// Folder the executable was first seen in.
    pub exe_dir: String,
    /// Folder of the runtime data (config, index, thumbnails).
    pub data_dir: String,
    /// True when a `portable` marker sat beside the exe: the user owns this
    /// folder, so nothing may delete it.
    pub portable: bool,
}

/// Writes the record on first run; keeps the first answer afterwards.
///
/// "First run" matters: a later run from a different folder (a copy someone
/// moved) must not erase the memory that the original folder was the user's.
pub fn remember_install() {
    if record_path().is_file() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else {
        return;
    };
    let record = InstallRecord {
        exe_dir: dir.to_string_lossy().to_string(),
        data_dir: KensakuConfig::config_dir().to_string_lossy().to_string(),
        portable: PORTABLE_MARKERS
            .iter()
            .any(|marker| dir.join(marker).is_file()),
    };
    if let Some(parent) = record_path().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(&record) {
        let _ = std::fs::write(record_path(), json);
    }
}

/// The recorded install, if any.
pub fn install_record() -> Option<InstallRecord> {
    let raw = std::fs::read_to_string(record_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

/// True when the running copy sits in a folder the user owns.
///
/// A folder is the user's own when it carries a portable marker, or when it is
/// anything other than the standard install location -- a folder someone
/// unpacked a zip into, a network share, a USB stick. The installer must not be
/// pointed at those: its first act is to uninstall, and its uninstall deletes
/// the install directory.
pub fn folder_belongs_to_user(exe_dir: &Path) -> bool {
    if PORTABLE_MARKERS
        .iter()
        .any(|marker| exe_dir.join(marker).is_file())
    {
        return true;
    }
    // The data folder living inside the same directory is the portable layout
    // (see `resolve_config_dir`): the index and thumbnails are in there too.
    if exe_dir.join("data").join("index").is_dir() {
        return true;
    }
    // A folder Kadr did not create is not Kadr's to delete.
    !exe_dir
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.eq_ignore_ascii_case("kadr"))
        .unwrap_or(false)
}

/// Why the installer must not be used here, or `None` when it is safe.
///
/// Returned to the UI so the update screen can explain itself instead of
/// silently doing the dangerous thing or silently doing nothing.
pub fn installer_refusal() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    if folder_belongs_to_user(dir) {
        return Some(format!(
            "this copy lives in {}, a folder that holds files you put there. Updating it with the \
             installer would uninstall the previous version first and delete that folder, so the \
             update stops here instead of risking your files. Move the folder somewhere writable \
             and update in place, or replace the exe by hand.",
            dir.display()
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kadr-install-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// A directory named `kadr` is the only one an uninstaller may remove.
    #[test]
    fn only_a_plain_kadr_folder_may_be_deleted() {
        let base = temp_dir("plain");
        let plain = base.join("kadr");
        std::fs::create_dir_all(&plain).unwrap();
        assert!(
            !folder_belongs_to_user(&plain),
            "the standard install folder is Kadr's own"
        );

        let mine = base.join("Мои документы");
        std::fs::create_dir_all(&mine).unwrap();
        assert!(
            folder_belongs_to_user(&mine),
            "a folder with any other name is the user's, never the uninstaller's"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The portable layout keeps data beside the exe; that is a user folder
    /// even when it happens to be called `kadr`.
    #[test]
    fn portable_layout_is_a_user_folder_even_when_named_kadr() {
        let base = temp_dir("portable");
        let dir = base.join("kadr");
        std::fs::create_dir_all(dir.join("data").join("index")).unwrap();
        std::fs::write(dir.join("portable"), b"1").unwrap();
        assert!(folder_belongs_to_user(&dir));
        let _ = std::fs::remove_dir_all(&base);
    }
}
