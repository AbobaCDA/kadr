//! The collection ("skill"): move the file you are looking at into a folder you
//! picked once.
//!
//! The workflow this exists for: search your library, find the shot you want,
//! press one key, and it leaves the haystack for a working folder. Doing that
//! by hand is four steps (open containing folder, copy, navigate, paste) per
//! file, which is the whole reason a local search tool is faster than a file
//! manager until you actually need the file somewhere.
//!
//! Design rules:
//!
//! - **Nothing is ever overwritten.** A name collision becomes `name (2).ext`,
//!   so a second press on a same-named file from another folder cannot destroy
//!   the first.
//! - **Moves survive a device boundary.** `rename` is atomic and free within a
//!   filesystem; across one (C: → D:, or two mounts) it fails with `EXDEV` and
//!   the file is copied, then removed, in that order — the source only
//!   disappears once the destination is verified.
//! - **Only media is collectable.** A hotkey that can silently relocate a
//!   `.docx` or a folder is a footgun; images and video are what the feature is
//!   for.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Image extensions the indexer also understands (thumbnails, OCR, phash).
const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "heic", "avif", "ico", "svg",
];

/// Video extensions. Must stay in sync with the `media` bucket in
/// `search::classify_extension`, minus audio: a music file is not what the
/// hotkey is for, and accidentally moving an album is expensive.
const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "avi", "mkv", "webm", "m4v", "mpg", "mpeg", "wmv", "flv", "3gp", "ts", "mts",
];

/// Whether the hotkey may act on this file.
pub fn is_collectable(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .is_some_and(|ext| is_collectable_extension(&ext))
}

pub fn is_collectable_extension(ext: &str) -> bool {
    let ext = ext.to_ascii_lowercase();
    IMAGE_EXTENSIONS.contains(&ext.as_str()) || VIDEO_EXTENSIONS.contains(&ext.as_str())
}

/// Copy or move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectMode {
    Move,
    Copy,
}

impl CollectMode {
    /// Parses the config value; anything unrecognised moves, because that is
    /// what the feature is documented to do.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "copy" => Self::Copy,
            _ => Self::Move,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Copy => "copy",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CollectOutcome {
    /// Path the file had before the skill ran.
    pub from: String,
    /// Path it has now.
    pub to: String,
    /// Destination folder, for the toast's "show folder" action.
    pub dir: String,
    /// `"move"` or `"copy"`, echoed back so the UI can word its message.
    pub mode: String,
}

/// Why a collect did not happen. Separate variants so the UI can react
/// differently: an unset folder is a prompt, everything else is a message.
#[derive(Debug)]
pub enum CollectError {
    /// No destination folder configured yet — the UI must ask for one.
    NoDestination,
    /// The file is not an image or a video.
    NotCollectable(String),
    /// The source is missing or not a file.
    Missing(String),
    /// The file already lives in the destination folder.
    AlreadyThere(String),
    /// Filesystem failure, already formatted for a human.
    Io(String),
}

impl CollectError {
    /// Stable machine-readable code, consumed by the frontend.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoDestination => "COLLECT_DIR_UNSET",
            Self::NotCollectable(_) => "COLLECT_NOT_MEDIA",
            Self::Missing(_) => "COLLECT_MISSING",
            Self::AlreadyThere(_) => "COLLECT_ALREADY_THERE",
            Self::Io(_) => "COLLECT_IO",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::NoDestination => {
                "No collection folder has been chosen yet; pick one to enable the skill.".to_string()
            }
            Self::NotCollectable(name) => {
                format!("{name} is not an image or a video, so it is not collected.")
            }
            Self::Missing(path) => format!("{path} no longer exists."),
            Self::AlreadyThere(path) => format!("{path} is already in the collection folder."),
            Self::Io(text) => text.clone(),
        }
    }
}

impl std::fmt::Display for CollectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

/// Moves (or copies) `source` into `dest_dir`, creating the folder when needed.
///
/// Returns the path the file ended up at.
pub fn collect(
    source: &Path,
    dest_dir: &Path,
    mode: CollectMode,
) -> Result<CollectOutcome, CollectError> {
    if dest_dir.as_os_str().is_empty() {
        return Err(CollectError::NoDestination);
    }
    if !source.is_file() {
        return Err(CollectError::Missing(
            source.to_string_lossy().to_string(),
        ));
    }
    if !is_collectable(source) {
        let name = source
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| source.to_string_lossy().to_string());
        return Err(CollectError::NotCollectable(name));
    }

    let dest_dir = absolute(dest_dir);
    fs::create_dir_all(&dest_dir).map_err(|error| {
        CollectError::Io(format!(
            "Could not create {}: {error}",
            dest_dir.to_string_lossy()
        ))
    })?;

    // Same folder: a move would be a no-op and a copy would duplicate the file
    // next to itself. Both are surprises, so refuse instead.
    let source_abs = absolute(source);
    if source_abs.parent() == Some(dest_dir.as_path()) {
        return Err(CollectError::AlreadyThere(
            source_abs.to_string_lossy().to_string(),
        ));
    }

    let target = unique_target(&dest_dir, &source_abs);
    match mode {
        CollectMode::Move => move_file(&source_abs, &target)?,
        CollectMode::Copy => copy_file(&source_abs, &target)?,
    }

    Ok(CollectOutcome {
        from: source_abs.to_string_lossy().to_string(),
        to: target.to_string_lossy().to_string(),
        dir: dest_dir.to_string_lossy().to_string(),
        mode: mode.as_str().to_string(),
    })
}

/// `dest_dir / name`, with ` (2)`, ` (3)`, … appended until it does not exist.
///
/// Deliberately never reuses a name: the point of a collection folder is that
/// everything in it survives.
fn unique_target(dest_dir: &Path, source: &Path) -> PathBuf {
    let file_name = source
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let (stem, ext) = match file_name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{ext}")),
        _ => (file_name.clone(), String::new()),
    };

    let mut candidate = dest_dir.join(&file_name);
    let mut counter = 2u32;
    while candidate.exists() {
        candidate = dest_dir.join(format!("{stem} ({counter}){ext}"));
        counter += 1;
        if counter > 10_000 {
            // Pathological: give up on a friendly name rather than spin.
            return dest_dir.join(format!(
                "{stem} ({} copies){ext}",
                chrono_like_stamp(std::time::SystemTime::now())
            ));
        }
    }
    candidate
}

fn chrono_like_stamp(now: std::time::SystemTime) -> u64 {
    now.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `rename` first: atomic and instant inside one filesystem. Across devices it
/// fails with `EXDEV`, and the fallback copies before it deletes.
fn move_file(source: &Path, target: &Path) -> Result<(), CollectError> {
    match fs::rename(source, target) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            copy_file(source, target)?;
            fs::remove_file(source).map_err(|error| {
                CollectError::Io(format!(
                    "Copied to {} but could not remove the original {}: {error}",
                    target.to_string_lossy(),
                    source.to_string_lossy()
                ))
            })
        }
        Err(error) => Err(CollectError::Io(format!(
            "Could not move {} to {}: {error}",
            source.to_string_lossy(),
            target.to_string_lossy()
        ))),
    }
}

fn copy_file(source: &Path, target: &Path) -> Result<(), CollectError> {
    fs::copy(source, target).map(|_| ()).map_err(|error| {
        // A half-written destination is worse than none: remove it so a retry
        // does not produce "file (2).png" beside a truncated "file.png".
        let _ = fs::remove_file(target);
        CollectError::Io(format!(
            "Could not copy {} to {}: {error}",
            source.to_string_lossy(),
            target.to_string_lossy()
        ))
    })
}

/// Makes `path` absolute without touching the filesystem, so a relative
/// destination is compared and reported consistently with the source.
fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return normalise(path);
    }
    match std::env::current_dir() {
        Ok(cwd) => normalise(&cwd.join(path)),
        Err(_) => path.to_path_buf(),
    }
}

/// Drops `.` components and resolves `..` lexically (no symlink resolution:
/// the paths here come from an index that already stores absolute paths).
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component.as_os_str());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kadr-collect-{tag}-{}-{}",
            std::process::id(),
            chrono_like_stamp(std::time::SystemTime::now())
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).expect("write fixture");
        path
    }

    #[test]
    fn media_is_collectable_and_documents_are_not() {
        for ext in ["png", "JPG", "webp", "heic", "avif", "svg", "mp4", "MOV", "mkv"] {
            assert!(
                is_collectable_extension(ext),
                "{ext} must be collectable"
            );
        }
        for ext in ["pdf", "docx", "rs", "zip", "mp3", "wav", ""] {
            assert!(
                !is_collectable_extension(ext),
                "{ext} must not be collectable"
            );
        }
    }

    #[test]
    fn move_takes_the_file_out_of_the_source_folder() {
        let src = temp_dir("move-src");
        let dest = temp_dir("move-dest");
        let file = write(&src, "shot.png", "pixels");

        let outcome = collect(&file, &dest, CollectMode::Move).expect("move");
        assert!(!file.exists(), "the source must be gone after a move");
        assert!(dest.join("shot.png").is_file());
        assert_eq!(outcome.mode, "move");
        assert_eq!(PathBuf::from(&outcome.to), dest.join("shot.png"));
    }

    #[test]
    fn copy_leaves_the_source_in_place() {
        let src = temp_dir("copy-src");
        let dest = temp_dir("copy-dest");
        let file = write(&src, "clip.mp4", "frames");

        collect(&file, &dest, CollectMode::Copy).expect("copy");
        assert!(file.is_file(), "a copy must not touch the source");
        assert_eq!(fs::read(dest.join("clip.mp4")).unwrap(), b"frames");
    }

    #[test]
    fn a_name_collision_never_overwrites() {
        let src = temp_dir("dup-src");
        let dest = temp_dir("dup-dest");
        let file = write(&src, "shot.png", "new pixels");
        fs::write(dest.join("shot.png"), "old pixels").unwrap();

        let outcome = collect(&file, &dest, CollectMode::Copy).expect("copy");
        assert_eq!(
            fs::read_to_string(dest.join("shot.png")).unwrap(),
            "old pixels",
            "the existing file must be untouched"
        );
        assert_eq!(
            PathBuf::from(&outcome.to),
            dest.join("shot (2).png"),
            "the new file gets a free name"
        );

        // And a third one keeps counting instead of reusing (2).
        let again = write(&src, "shot.png", "third");
        let outcome = collect(&again, &dest, CollectMode::Copy).expect("copy");
        assert_eq!(PathBuf::from(&outcome.to), dest.join("shot (3).png"));
    }

    #[test]
    fn the_destination_folder_is_created_on_demand() {
        let src = temp_dir("makedir-src");
        let base = temp_dir("makedir-dest");
        let dest = base.join("nested/collection");
        let file = write(&src, "shot.png", "pixels");

        collect(&file, &dest, CollectMode::Move).expect("move");
        assert!(dest.join("shot.png").is_file());
    }

    #[test]
    fn documents_are_rejected_before_anything_is_touched() {
        let src = temp_dir("reject-src");
        let dest = temp_dir("reject-dest");
        let file = write(&src, "contract.pdf", "text");

        let error = collect(&file, &dest, CollectMode::Move).expect_err("must refuse");
        assert_eq!(error.code(), "COLLECT_NOT_MEDIA");
        assert!(file.is_file(), "a refused file must not move");
        assert!(!dest.join("contract.pdf").exists());
    }

    #[test]
    fn a_missing_source_reports_missing() {
        let src = temp_dir("missing-src");
        let dest = temp_dir("missing-dest");
        let error = collect(&src.join("ghost.png"), &dest, CollectMode::Move)
            .expect_err("must refuse");
        assert_eq!(error.code(), "COLLECT_MISSING");
    }

    #[test]
    fn collecting_into_the_same_folder_is_refused() {
        let src = temp_dir("same-src");
        let file = write(&src, "shot.png", "pixels");
        let error =
            collect(&file, &src, CollectMode::Copy).expect_err("same folder must be refused");
        assert_eq!(error.code(), "COLLECT_ALREADY_THERE");
        assert!(!src.join("shot (2).png").exists());
    }

    #[test]
    fn an_unset_destination_asks_the_ui_to_prompt() {
        let src = temp_dir("unset-src");
        let file = write(&src, "shot.png", "pixels");
        let error = collect(&file, Path::new(""), CollectMode::Move).expect_err("must refuse");
        assert_eq!(error.code(), "COLLECT_DIR_UNSET");
    }

    #[test]
    fn normalise_drops_dots_without_a_filesystem() {
        assert_eq!(normalise(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
        assert_eq!(normalise(Path::new("/a/b/")), PathBuf::from("/a/b"));
    }
}
