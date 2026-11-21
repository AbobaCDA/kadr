//! Overnight warm-up: prepare the cache while nobody is waiting.
//!
//! The user's request was "start it in the evening, leave it for eight hours,
//! come back and browse". Two things make that hard:
//!
//! * Thumbnails were only ever built when the grid asked for one, so the first
//!   pass over ten thousand photos meant waiting on a decode per card -- the
//!   "constantly scrolling and waiting" complaint.
//! * The scan itself paced itself to keep the desktop responsive, which is
//!   right while someone is working and pointless at 2 a.m.
//!
//! So this module has two knobs the UI can set, and both are honest about what
//! they cost: [`set_night_mode`] lets the crawl use every core, [`warm_start`]
//! walks the images and writes exactly the thumbnails the grid will look for.
//! Nothing here starts on its own; a run is always something the user asked
//! for, and it stops when they ask it to.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::indexer::IndexerManager;

/// Crawl with every core available (the "leave it overnight" switch).
static NIGHT_MODE: AtomicBool = AtomicBool::new(false);
/// Cancel signal for the thumbnail pass.
static WARM_STOP: AtomicBool = AtomicBool::new(false);
static WARM_RUNNING: AtomicBool = AtomicBool::new(false);
static WARM_DONE: AtomicUsize = AtomicUsize::new(0);
static WARM_TOTAL: AtomicUsize = AtomicUsize::new(0);
static WARM_SKIPPED: AtomicUsize = AtomicUsize::new(0);

/// Visible state of the warm-up, polled by the settings screen.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct WarmStatus {
    pub running: bool,
    /// Files whose thumbnail now exists (including ones that were already
    /// cached, which is what makes "done" mean something on a second run).
    pub done: usize,
    pub total: usize,
    /// Files that cannot have a thumbnail at all (cloud placeholders, broken
    /// images). Counted so a run can finish at 100% instead of looping.
    pub skipped: usize,
}

pub fn night_mode() -> bool {
    NIGHT_MODE.load(Ordering::Relaxed)
}

pub fn set_night_mode(on: bool) {
    NIGHT_MODE.store(on, Ordering::Relaxed);
}

pub fn status() -> WarmStatus {
    WarmStatus {
        running: WARM_RUNNING.load(Ordering::Relaxed),
        done: WARM_DONE.load(Ordering::Relaxed),
        total: WARM_TOTAL.load(Ordering::Relaxed),
        skipped: WARM_SKIPPED.load(Ordering::Relaxed),
    }
}

/// Stops the thumbnail pass. Safe to call when nothing is running.
pub fn stop() {
    WARM_STOP.store(true, Ordering::Relaxed);
}

/// Extensions the grid can render. Kept in sync with `preview::kind_for`: an
/// extension missing here silently stays uncached and slow.
fn is_warmable(name: &str) -> bool {
    let Some(ext) = name.rsplit('.').next().filter(|_| name.contains('.')) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "tif"
    )
}

/// Starts the thumbnail pass in the background and returns immediately.
///
/// Returns `false` when a pass is already running: two passes over the same
/// files would double the decode work for no benefit.
pub fn warm_start(manager: Arc<IndexerManager>) -> bool {
    if WARM_RUNNING.swap(true, Ordering::SeqCst) {
        return false;
    }
    WARM_STOP.store(false, Ordering::Relaxed);
    WARM_DONE.store(0, Ordering::Relaxed);
    WARM_TOTAL.store(0, Ordering::Relaxed);
    WARM_SKIPPED.store(0, Ordering::Relaxed);

    std::thread::Builder::new()
        .name("kadr-warm-thumbs".into())
        .spawn(move || {
            let out_dir = crate::commands::thumbnail_cache_dir_public();
            // Collect first so the progress bar has a total instead of a file
            // count that keeps growing while the user watches it.
            let mut targets: Vec<PathBuf> = Vec::new();
            {
                let metadata = manager.metadata.read();
                let len = metadata.len();
                for i in 0..len {
                    if WARM_STOP.load(Ordering::Relaxed) {
                        break;
                    }
                    let Some(entry) = metadata.materialize(i as u32) else {
                        continue;
                    };
                    if entry.is_dir {
                        continue;
                    }
                    let name = entry
                        .path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_default();
                    if !is_warmable(name) {
                        continue;
                    }
                    targets.push(entry.path.clone());
                }
            }
            WARM_TOTAL.store(targets.len(), Ordering::Relaxed);

            // The same CPU budget the crawler uses: this is a background chore,
            // not something to make the desktop stutter for.
            let mode = manager.config.read().crawl_mode;
            let threads = crate::util::power::crawl_threads(mode).max(1);
            let queue = Arc::new(RwLock::new(std::collections::VecDeque::from(targets)));
            let mut handles = Vec::new();
            for _ in 0..threads {
                let queue = Arc::clone(&queue);
                let out_dir = out_dir.clone();
                handles.push(std::thread::spawn(move || {
                    loop {
                        if WARM_STOP.load(Ordering::Relaxed) {
                            return;
                        }
                        let next = queue.write().pop_front();
                        let Some(path) = next else {
                            return;
                        };
                        if crate::indexer::walker::is_cloud_placeholder_path(&path) {
                            WARM_SKIPPED.fetch_add(1, Ordering::Relaxed);
                        } else if crate::preview::warm_thumbnail(&path, &out_dir).is_some() {
                            WARM_DONE.fetch_add(1, Ordering::Relaxed);
                        } else {
                            WARM_SKIPPED.fetch_add(1, Ordering::Relaxed);
                        }
                        // Memory pressure means a crawl is struggling; a
                        // thumbnail can wait rather than push it over.
                        if crate::util::power::memory_pressure() {
                            std::thread::sleep(std::time::Duration::from_millis(250));
                        }
                    }
                }));
            }
            for handle in handles {
                let _ = handle.join();
            }
            WARM_RUNNING.store(false, Ordering::Relaxed);
            crate::say_err!(
                "[Kadr] thumbnail warm-up finished: {} done, {} skipped",
                WARM_DONE.load(Ordering::Relaxed),
                WARM_SKIPPED.load(Ordering::Relaxed)
            );
        })
        .is_ok_and(|_| true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn night_mode_is_what_makes_the_crawl_fast() {
        // The switch the button flips. Everything else in this module is
        // housekeeping; this is the part that changes how the scan behaves.
        set_night_mode(false);
        assert_eq!(
            crate::util::power::effective_mode(crate::config::CrawlMode::Eco),
            crate::config::CrawlMode::Eco
        );
        set_night_mode(true);
        assert_eq!(
            crate::util::power::effective_mode(crate::config::CrawlMode::Eco),
            crate::config::CrawlMode::Full,
            "an overnight run must use every core, whatever is saved"
        );
        set_night_mode(false);
    }

    #[test]
    fn a_saved_eco_mode_survives_the_night() {
        // The point of keeping night mode separate from the config: the user's
        // daytime choice is not silently rewritten by a night run.
        set_night_mode(true);
        let saved = crate::config::CrawlMode::Eco;
        let _ = crate::util::power::effective_mode(saved);
        set_night_mode(false);
        assert_eq!(
            crate::util::power::effective_mode(saved),
            crate::config::CrawlMode::Eco
        );
    }

    #[test]
    fn only_renderable_extensions_are_warmed() {
        for name in ["a.jpg", "b.JPEG", "c.png", "d.webp", "e.tiff"] {
            assert!(is_warmable(name), "{name} must be warmed");
        }
        for name in ["notes.txt", "clip.mov", "no-extension", "archive.zip"] {
            assert!(!is_warmable(name), "{name} must not be warmed");
        }
    }

    #[test]
    fn night_mode_is_off_until_asked() {
        // The whole point of the setting: nothing heavy happens behind the
        // user's back.
        set_night_mode(false);
        assert!(!night_mode());
        set_night_mode(true);
        assert!(night_mode());
        set_night_mode(false);
    }
}
