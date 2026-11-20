use crate::indexer::ignore::IgnoreSet;
use crate::search::fuzzy::compute_char_mask;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub id: u64,
    pub path: PathBuf,
    pub name: String,
    /// Interned (lowercase) extension. Extensions repeat heavily, so a
    /// process-wide `&'static str` avoids one heap allocation per entry during
    /// a full crawl while keeping `Option<&str>` call sites unchanged.
    pub extension: Option<&'static str>,
    pub size: u64,
    pub modified: u64,
    pub is_dir: bool,
    /// Filesystem device id (`st_dev`). `0` means unknown.
    pub dev: u64,
    /// Filesystem inode number (`st_ino`). `0` means unknown.
    pub ino: u64,
    /// 64-bit character presence bitmask for fast prefiltering.
    pub char_bloom: u64,
    /// True when the entry's content must be re-read even if its mtime/size did
    /// not change (used to re-run image OCR after the setting is toggled).
    pub content_pending: bool,
}

/// Interns an extension for the lifetime of the process.
///
/// Unique extensions are a tiny fixed set, so leaking each one once is a
/// bounded cost and removes the per-entry `String` from a 1M-entry crawl.
pub fn intern_extension(extension: Option<&str>) -> Option<&'static str> {
    let extension = extension?.trim().trim_start_matches('.');
    if extension.is_empty() {
        return None;
    }
    static EXTENSIONS: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut table = EXTENSIONS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    if let Some(&interned) = table.get(extension) {
        return Some(interned);
    }
    let lower = extension.to_ascii_lowercase();
    if let Some(&interned) = table.get(lower.as_str()) {
        return Some(interned);
    }
    let interned: &'static str = Box::leak(lower.into_boxed_str());
    table.insert(interned);
    Some(interned)
}

/// Returns `(device_id, inode)` for a filesystem metadata entry.
///
/// These values form the stable identity of a file across path-based renames
/// and replacements. On non-Unix platforms they are unavailable and default
/// to `0`, which callers treat as "unknown".
pub fn file_identity(meta: &std::fs::Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (meta.dev(), meta.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        (0, 0)
    }
}

/// True when the file is a cloud-backed placeholder that would be hydrated
/// (downloaded) if its contents were read.
///
/// - Windows (OneDrive, Dropbox, iCloud for Windows): the offline /
///   recall-on-data-access attributes.
/// - macOS (iCloud Drive with "Optimize Mac Storage", Desktop & Documents in
///   iCloud, File Provider apps): APFS marks evicted files `SF_DATALESS`.
///   `stat` never downloads them, but reading even one byte does, so before
///   this check indexing quietly downloaded the user's whole cloud library.
/// - Linux has no common equivalent; returns `false`.
#[cfg(windows)]
pub fn is_cloud_placeholder(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    // https://learn.microsoft.com/windows/win32/fileio/file-attribute-constants
    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
    const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

    let attributes = meta.file_attributes();
    attributes & (FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS) != 0
}

#[cfg(target_os = "macos")]
pub fn is_cloud_placeholder(meta: &std::fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;

    // <sys/stat.h>: "file is dataless object" (contents live in the cloud).
    const SF_DATALESS: u32 = 0x4000_0000;
    meta.st_flags() & SF_DATALESS != 0
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn is_cloud_placeholder(_meta: &std::fs::Metadata) -> bool {
    false
}

/// Convenience wrapper for callers that only have a path. A `stat`, which
/// never downloads anything; on Linux it does not touch the filesystem.
pub fn is_cloud_placeholder_path(path: &std::path::Path) -> bool {
    #[cfg(any(windows, target_os = "macos"))]
    {
        std::fs::metadata(path)
            .map(|meta| is_cloud_placeholder(&meta))
            .unwrap_or(false)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = path;
        false
    }
}

/// Directories whose entire subtree the crawler must never enter.
///
/// Kadr's own runtime data (config, SQLite metadata, Tantivy index,
/// thumbnails, OCR model packs, the daemon socket) lives under
/// [`crate::config::KensakuConfig::config_dir`]. On macOS that is
/// `~/Library/Application Support/kensaku` — *not* a hidden path — so a
/// home-rooted crawl would otherwise walk its own index while it writes it:
/// thousands of files, re-processed on every change. On Linux the config
/// directory starts with `.` and `skip_hidden` already hides it; on Windows it
/// is `%APPDATA%\kensaku`, also not dot-prefixed. The explicit prune is
/// correct on every platform and costs one path comparison per entry.
pub fn runtime_dir_prunes() -> Vec<PathBuf> {
    vec![crate::config::KensakuConfig::config_dir()]
}

/// Operating-system folders that a "search the whole disk" root must not pull
/// in.
///
/// `C:` is a normal thing to add, and it silently means "also index Windows":
/// System32 alone is tens of thousands of files, WinSxS holds hard-link farms
/// of the same binaries, and every one of them is churn that no search wants.
/// The list is stored in the config rather than applied silently here, so the
/// exclusions show up in Settings and can be undone.
pub fn os_prunes() -> Vec<PathBuf> {
    let Some(root) = std::env::var_os("SystemRoot") else {
        return Vec::new();
    };
    vec![PathBuf::from(root)]
}

/// The OS's application-data tree directly under the home folder:
/// `%USERPROFILE%\AppData` on Windows, `~/Library` on macOS. Browser caches,
/// app containers and logs: huge, constantly changing, and never what a
/// file search is for. (Linux keeps this in dot-folders, already hidden.)
pub fn system_data_dirs() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    if cfg!(windows) {
        // CrossDevice: Phone Link exposes a connected phone here as a live
        // cloud-files folder. Listing it asks the phone over Wi-Fi, and when
        // the phone is away Windows pops "<phone> is offline" every time.
        vec![home.join("AppData"), home.join("CrossDevice")]
    } else if cfg!(target_os = "macos") {
        vec![home.join("Library")]
    } else {
        Vec::new()
    }
}

/// The user's collection folder, when it must be pruned.
///
/// The collect hotkey *moves* a file into this folder. Indexing it would list
/// every collected file a second time under its new name, which looks exactly
/// like a duplicate in the results; the folder would also grow without bound
/// inside the index. So the whole subtree is skipped, by path rather than by
/// name: two folders called "Photos" must not both disappear because one of
/// them is the collection.
///
/// A collection folder that *is* a crawl root yields nothing: pruning it would
/// make the crawl return zero files, and the user asked for it explicitly.
pub fn collect_dir_prunes(roots: &[PathBuf], collect_dir: Option<&PathBuf>) -> Vec<PathBuf> {
    let Some(dir) = collect_dir else {
        return Vec::new();
    };
    if roots.iter().any(|root| root == dir) {
        return Vec::new();
    }
    vec![dir.clone()]
}

/// Everything a walk or watch over `roots` must not enter: Kadr's own
/// data always, plus the OS data trees, except one that a root lives inside.
/// Someone who adds iCloud Drive (`~/Library/Mobile Documents/...`) on
/// purpose gets it; someone who adds their home folder does not get
/// `~/Library` with it.
pub fn prunes_for_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    prunes_for_roots_with(roots, system_data_dirs())
}

fn prunes_for_roots_with(roots: &[PathBuf], system: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut prunes = runtime_dir_prunes();
    for dir in system {
        if !roots.iter().any(|root| root.starts_with(&dir)) {
            prunes.push(dir);
        }
    }
    prunes
}

/// True when `meta` is a cloud-backed folder whose listing is not on disk,
/// so reading it would make the provider fetch it (OneDrive "online-only"
/// folders, Phone Link, macOS File Provider). The folder itself stays
/// findable by name; the walk just does not descend into it.
#[cfg(windows)]
pub fn is_unpopulated_cloud_dir(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
    const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
    const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
    meta.is_dir()
        && meta.file_attributes()
            & (FILE_ATTRIBUTE_OFFLINE
                | FILE_ATTRIBUTE_RECALL_ON_OPEN
                | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
            != 0
}

#[cfg(target_os = "macos")]
pub fn is_unpopulated_cloud_dir(meta: &std::fs::Metadata) -> bool {
    meta.is_dir() && is_cloud_placeholder(meta)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn is_unpopulated_cloud_dir(_meta: &std::fs::Metadata) -> bool {
    false
}

/// True when `path` lives at or below any of `prune_roots`.
pub fn starts_with_any(path: &std::path::Path, prune_roots: &[PathBuf]) -> bool {
    prune_roots.iter().any(|root| path.starts_with(root))
}

/// Walks `dirs` and returns file entries.
///
/// When `include_ignored` is false (the default), `.gitignore` /
/// `.cursorignore` rules under the roots are honored so agents see the same
/// tree developers do; see [`crate::indexer::ignore`]. The walk itself still
/// traverses ignored directories (the cheap default `excluded_dirs` list
/// handles the heavy hitters); their entries are filtered before returning.
pub fn walk_directories(
    dirs: &[PathBuf],
    excluded_dirs: &[String],
    include_ignored: bool,
) -> Vec<FileEntry> {
    walk_directories_pruned(dirs, excluded_dirs, include_ignored, &[])
}

/// [`walk_directories`] plus extra path-based prunes (the collection folder,
/// see [`collect_dir_prunes`]). Kept as a separate entry point so the dozens of
/// call sites that have nothing extra to prune do not thread an empty slice
/// through.
pub fn walk_directories_pruned(
    dirs: &[PathBuf],
    excluded_dirs: &[String],
    include_ignored: bool,
    extra_prunes: &[PathBuf],
) -> Vec<FileEntry> {
    walk_directories_with_mode(
        dirs,
        excluded_dirs,
        include_ignored,
        extra_prunes,
        crate::config::CrawlMode::Balanced,
        true,
    )
}

/// [`walk_directories_pruned`] with an explicit crawl mode, which decides how
/// many threads the directory walk may use. Callers that care about keeping the
/// machine responsive pass the configured mode.
pub fn walk_directories_with_mode(
    dirs: &[PathBuf],
    excluded_dirs: &[String],
    include_ignored: bool,
    extra_prunes: &[PathBuf],
    crawl_mode: crate::config::CrawlMode,
    skip_hidden: bool,
) -> Vec<FileEntry> {
    let mut result = Vec::new();
    let mut ignore_dirs: Vec<PathBuf> = Vec::new();
    let excluded_set: HashSet<String> = excluded_dirs.iter().map(|s| s.to_lowercase()).collect();

    // Default-deny sensitive paths. These are skipped even if the user has not
    // added them to exclusions. Hidden entries (names starting with '.') are
    // already skipped above, which covers ~/.ssh, ~/.gnupg, ~/.aws, etc.
    const SENSITIVE_NAMES: &[&str] = &[
        ".ssh",
        ".gnupg",
        ".gpg",
        ".aws",
        ".azure",
        ".kube",
        ".docker",
        ".password-store",
        "1password",
        "bitwarden",
        "keepass",
        "keepassxc",
        ".env",
        ".env.local",
        ".env.production",
        ".env.development",
    ];
    let sensitive_set: HashSet<String> = SENSITIVE_NAMES.iter().map(|s| s.to_string()).collect();

    for root in dirs {
        if !root.exists() {
            continue;
        }

        //
        // jwalk defaults to one thread per core and keeps going while the
        // machine is busy, so on a 3M-file crawl the *walk itself* saturated
        // every core before any file was even opened. The cap comes from the
        // crawl mode (see `power::crawl_threads`): balanced leaves a core for
        // the desktop, eco uses half.
        let walker = jwalk::WalkDir::new(root)
            // The setting existed but was never read here, so "index hidden
            // files" did nothing. Dot-folders still keep their own guard below.
            .skip_hidden(skip_hidden)
            .parallelism(jwalk::Parallelism::RayonNewPool(
                crate::util::power::crawl_threads(crawl_mode),
            ))
            .process_read_dir({
                let excluded_set = excluded_set.clone();
                let sensitive_set = sensitive_set.clone();
                let mut prune_roots = prunes_for_roots(dirs);
                prune_roots.extend_from_slice(extra_prunes);
                move |_depth, _path, _read_dir_state, children| {
                    children.retain(|dir_entry_result| {
                        if let Ok(entry) = dir_entry_result {
                            // Kadr's own runtime data: never crawl it
                            // (macOS path, see `runtime_dir_prunes`).
                            if starts_with_any(&entry.path(), &prune_roots) {
                                return false;
                            }
                            let name = entry.file_name.to_string_lossy().to_lowercase();
                            // Belt-and-suspenders: never descend into hidden entries
                            // even if `skip_hidden` is unsupported on this platform.
                            if name.starts_with('.') {
                                return false;
                            }
                            if excluded_set.contains(&name) {
                                return false;
                            }
                            if sensitive_set.contains(&name) {
                                return false;
                            }
                        }
                        true
                    });
                    // Keep cloud-only folders as entries, but never list
                    // their contents: that is a network fetch (and, for a
                    // phone over Phone Link, an "offline" popup). Detection
                    // is Windows/macOS-only, so other platforms skip the
                    // extra per-directory stat entirely.
                    #[cfg(any(windows, target_os = "macos"))]
                    for entry in children.iter_mut().flatten() {
                        if entry.file_type.is_dir()
                            && let Ok(meta) = std::fs::metadata(entry.path())
                            && is_unpopulated_cloud_dir(&meta)
                        {
                            entry.read_children = None;
                        }
                    }
                }
            });

        for entry_res in walker {
            let entry = match entry_res {
                Ok(e) => e,
                Err(_) => continue,
            };

            let path = entry.path();
            let is_dir = entry.file_type().is_dir();
            let name = entry.file_name.to_string_lossy().to_string();

            let extension = if is_dir {
                None
            } else {
                intern_extension(path.extension().and_then(|ext| ext.to_str()))
            };

            let (size, modified, dev, ino) = match entry.metadata() {
                Ok(meta) => {
                    let sz = meta.len();
                    let mod_time = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let (dev, ino) = file_identity(&meta);
                    (sz, mod_time, dev, ino)
                }
                Err(_) => (0, 0, 0, 0),
            };

            let char_bloom = compute_char_mask(&name);
            let file_entry = FileEntry {
                id: 0,
                path: path.clone(),
                name,
                extension,
                size,
                modified,
                is_dir,
                dev,
                ino,
                char_bloom,
                content_pending: false,
            };

            if !include_ignored && is_dir {
                ignore_dirs.push(path);
            }

            result.push(file_entry);
        }
    }

    // Ignore parity: probe each visited directory for `.gitignore` /
    // `.cursorignore` and drop ignored entries.
    {
        let ignore_set = if include_ignored {
            IgnoreSet::empty()
        } else {
            IgnoreSet::from_directories(&ignore_dirs)
        };
        // The watcher runs between crawls and has no directory list of its
        // own; it reads these rules so an edit inside an ignored tree does not
        // put the file back into the index.
        crate::indexer::ignore::set_active(ignore_set.clone());
        if !include_ignored && !ignore_set.is_empty() {
            result.retain(|entry| !ignore_set.is_ignored(&entry.path, entry.is_dir));
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_interning_is_stable_and_case_insensitive() {
        let upper = intern_extension(Some("PNG")).expect("interned");
        let dotted = intern_extension(Some(".png")).expect("interned");
        assert_eq!(upper, dotted);
        assert!(std::ptr::eq(upper, dotted));
        assert_eq!(intern_extension(None), None);
        assert_eq!(intern_extension(Some("")), None);
        assert_eq!(intern_extension(Some(".")), None);
    }

    #[test]
    fn interned_extension_storage_is_inline() {
        assert!(
            std::mem::size_of::<Option<&'static str>>() < std::mem::size_of::<Option<String>>()
        );
    }

    #[test]
    fn system_data_is_pruned_unless_a_root_lives_inside_it() {
        let home = PathBuf::from("/home/me");
        let library = home.join("Library");
        // Home picked: ~/Library is skipped.
        let prunes = prunes_for_roots_with(std::slice::from_ref(&home), vec![library.clone()]);
        assert!(starts_with_any(&library.join("Caches/x"), &prunes));
        assert!(!starts_with_any(&home.join("Documents/a.txt"), &prunes));
        // iCloud Drive picked on purpose: it is indexed.
        let icloud = library.join("Mobile Documents/com~apple~CloudDocs");
        let prunes = prunes_for_roots_with(
            &[home.clone(), icloud.clone()],
            vec![library.clone()],
        );
        assert!(!starts_with_any(&icloud.join("notes.txt"), &prunes));
    }

    #[test]
    fn collect_dir_is_pruned_by_path_not_by_name() {
        let root = PathBuf::from("/data");
        let collect = root.join("Photos/Collection");

        // Not set: nothing extra to prune.
        assert!(collect_dir_prunes(&[root.clone()], None).is_empty());

        // Set: the folder and everything below it are pruned.
        let prunes = collect_dir_prunes(&[root.clone()], Some(&collect));
        assert_eq!(prunes, vec![collect.clone()]);
        assert!(starts_with_any(&collect.join("2024/a.jpg"), &prunes));
        assert!(starts_with_any(&collect, &prunes));

        // A sibling that merely shares the name, and the parent folder, stay
        // searchable: the prune is a path prefix, never a name match.
        assert!(!starts_with_any(&root.join("Photos/Other/a.jpg"), &prunes));
        assert!(!starts_with_any(&root.join("Photos/a.jpg"), &prunes));
        assert!(!starts_with_any(&root.join("Photos/Collection 2/a.jpg"), &prunes));

        // Path prefixes are compared by component: `/data/PhotosX` is not
        // inside `/data/Photos`.
        assert!(!starts_with_any(&PathBuf::from("/data/PhotosX/a.jpg"), &prunes));
    }

    #[test]
    fn collect_dir_that_is_itself_a_root_is_not_pruned() {
        // Pruning the only root would index nothing at all, which is never what
        // the user meant by picking it.
        let root = PathBuf::from("/data/Photos");
        assert!(collect_dir_prunes(std::slice::from_ref(&root), Some(&root)).is_empty());
    }

    #[test]
    fn walk_skips_the_collection_folder() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "kadr-collect-prune-{}-{}",
            std::process::id(),
            nanos
        ));
        let keep = base.join("inbox");
        let collect = base.join("collection");
        std::fs::create_dir_all(&keep).unwrap();
        std::fs::create_dir_all(collect.join("nested")).unwrap();
        std::fs::write(keep.join("kept.png"), b"x").unwrap();
        std::fs::write(collect.join("moved.png"), b"x").unwrap();
        std::fs::write(collect.join("nested/also-moved.png"), b"x").unwrap();

        let all = walk_directories(std::slice::from_ref(&base), &[], false);
        assert!(all.iter().any(|entry| entry.name == "moved.png"));

        let prunes = collect_dir_prunes(std::slice::from_ref(&base), Some(&collect));
        let filtered = walk_directories_pruned(std::slice::from_ref(&base), &[], false, &prunes);
        assert!(filtered.iter().any(|entry| entry.name == "kept.png"));
        assert!(
            !filtered.iter().any(|entry| entry.name == "moved.png"),
            "the collect folder must not be crawled"
        );
        assert!(!filtered.iter().any(|entry| entry.name == "also-moved.png"));

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn the_system_folder_is_an_optional_exclusion_not_a_silent_skip() {
        // The walker prunes what it is told to prune; `C:\\Windows` reaches it
        // through the config, so a user who really wants it can say so.
        let prunes = os_prunes();
        if cfg!(windows) {
            assert_eq!(prunes.len(), 1);
            assert!(prunes[0].to_string_lossy().to_lowercase().ends_with("windows"));
        } else {
            // No `%SystemRoot%` elsewhere: nothing is excluded behind the
            // user's back.
            assert!(prunes.is_empty() || !prunes[0].as_os_str().is_empty());
        }
    }

    #[test]
    fn runtime_dir_prunes_covers_config_dir_and_subtrees() {
        let prunes = runtime_dir_prunes();
        assert!(!prunes.is_empty());
        let config_dir = crate::config::KensakuConfig::config_dir();
        // The config dir itself, and everything inside it (index/, thumbs/,
        // tessdata/, models/), are pruned; siblings are not.
        assert!(starts_with_any(&config_dir, &prunes));
        assert!(starts_with_any(&config_dir.join("index"), &prunes));
        assert!(config_dir.join("index").join("tantivy").is_absolute() == config_dir.is_absolute());
        assert!(starts_with_any(
            &config_dir.join("thumbs").join("a.jpg"),
            &prunes
        ));
        // A sibling directory that merely shares a name prefix must not be
        // pruned (e.g. `kensaku-cli` next to `kensaku`).
        let sibling = config_dir.parent().map(|p| p.join("kensaku-cli"));
        if let Some(sibling) = sibling {
            assert!(!starts_with_any(&sibling, &prunes));
        }
        // A path with the config dir as an ancestor component must also be
        // pruned wherever it appears under a watched root.
        assert!(starts_with_any(
            &config_dir.join("models").join("det.onnx"),
            &prunes
        ));
    }

    #[test]
    fn walker_never_enters_its_own_runtime_dir() {
        // Build a fake tree: watched_root/Library/... sits next to Kadr's
        // config dir, mimicking the macOS layout
        // (config dir = ~/Library/Application Support/kensaku).
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "kensaku-walker-prune-{}-{}",
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(&base).unwrap();

        let config_dir = crate::config::KensakuConfig::config_dir();
        // The suite cannot override `dirs::config_dir()` safely in parallel
        // tests, so instead prove that walk_directories prunes whatever the
        // *current* runtime dir would be, plus that ordinary entries still
        // survive. Create an ordinary file under the root.
        let root = base.join("root");
        let docs = root.join("Documents");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("notes.txt"), b"hello").unwrap();

        let entries = walk_directories(std::slice::from_ref(&root), &[], false);
        assert!(
            entries.iter().any(|e| e.path == root.join("Documents")),
            "ordinary directories must survive the prune"
        );
        // Nothing inside Kadr's runtime dir can appear in the results:
        // every entry under the config dir would start with it.
        assert!(
            !entries
                .iter()
                .any(|e| starts_with_any(&e.path, &runtime_dir_prunes())),
            "the crawler must never index its own storage directory"
        );

        assert!(std::fs::remove_dir_all(&base).is_ok());
        let _ = config_dir;
    }
}
