//! Whether a crawl has ever finished walking every watched folder.
//!
//! The answer is stored on disk next to the index, not in a process field: the
//! daemon, the GUI and a restarted app all have to agree on it, and "the index
//! is complete" outlives one run of the program.
//!
//! Why the UI needs it: while the first scan of a big disk is running, the
//! number of matching files keeps climbing. Any page the user has already
//! scrolled past was counted against a smaller index, so the pages do not add
//! up to the total and the grid has to keep paging -- forever, if the disk is
//! large enough. Once this flag is set, the pages do add up and the client can
//! fill the list in the background instead of asking the user to scroll for it.

use std::path::PathBuf;

use crate::config::KensakuConfig;

/// Marker written when a crawl finishes its walk.
///
/// Written after the commit, so a crash mid-crawl leaves no marker and the next
/// run knows the index is still partial.
fn marker_path() -> PathBuf {
    KensakuConfig::index_dir().join("crawl-complete")
}

/// Records that a crawl walked every watched folder.
pub fn mark_crawl_finished() {
    let path = marker_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, b"1") {
        crate::say_err!("[Kadr] could not record that the crawl finished: {e}");
    }
}

/// True once any crawl has finished walking all watched folders.
///
/// False means "the match count is still growing": the count is a snapshot, and
/// a client that wants the whole match set has to keep paging.
pub fn crawl_finished() -> bool {
    marker_path().is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The marker is per data directory, so a test run cannot see the marker a
    /// real index left behind (and vice versa).
    struct TempDataDir {
        previous: Option<std::ffi::OsString>,
        dir: PathBuf,
    }

    impl TempDataDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "kadr-crawl-state-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            let _ = std::fs::create_dir_all(&dir);
            let previous = std::env::var_os("KENSAKU_DATA_DIR");
            // SAFETY-ish: tests in this module share the process env, so this
            // is why the module has exactly one test touching it.
            unsafe { std::env::set_var("KENSAKU_DATA_DIR", &dir) };
            Self { previous, dir }
        }
    }

    impl Drop for TempDataDir {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe { std::env::set_var("KENSAKU_DATA_DIR", value) },
                None => unsafe { std::env::remove_var("KENSAKU_DATA_DIR") },
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn a_fresh_index_is_not_finished_and_a_marker_is_enough() {
        let _guard = TempDataDir::new("fresh");
        assert!(
            !crawl_finished(),
            "nothing has walked the disk yet, so the total is still growing"
        );
        mark_crawl_finished();
        assert!(crawl_finished(), "a finished crawl must be visible to search");
    }
}
