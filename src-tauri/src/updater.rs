use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use tauri::AppHandle;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub has_update: bool,
    pub release_name: String,
    pub release_notes: String,
    pub release_url: String,
    pub published_at: String,
    pub asset_name: Option<String>,
    pub asset_url: Option<String>,
    pub asset_size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateProgress {
    pub phase: String,
    pub downloaded: u64,
    pub total: u64,
    pub percent: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateStatus {
    pub busy: bool,
    pub progress: Option<UpdateProgress>,
    pub error: Option<String>,
    pub downloaded_path: Option<String>,
    pub ready_to_install: bool,
}

/// Compares two version strings (`candidate` vs `current`), stripping any leading
/// 'v' and discarding pre-release / build metadata tags (e.g. "1.0.14-beta").
/// Returns true only if `candidate` is strictly newer.
pub fn is_newer_version(candidate: &str, current: &str) -> bool {
    fn parse(v: &str) -> Vec<u64> {
        let clean = v.trim().trim_start_matches('v').split(['-', '+']).next().unwrap_or("");
        clean.split('.').filter_map(|s| s.parse::<u64>().ok()).collect()
    }
    let a = parse(candidate);
    let b = parse(current);
    let len = a.len().max(b.len());
    for i in 0..len {
        let val_a = a.get(i).copied().unwrap_or(0);
        let val_b = b.get(i).copied().unwrap_or(0);
        if val_a != val_b {
            return val_a > val_b;
        }
    }
    false
}

/// Matches the running OS and CPU architecture against the released assets.
pub fn select_platform_asset(assets: &[ReleaseAsset]) -> Option<ReleaseAsset> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    match os {
        "windows" => {
            let is_arm = arch == "aarch64" || arch == "arm64";
            if is_arm {
                if let Some(a) = assets.iter().find(|a| a.name.to_lowercase().contains("arm64") && a.name.ends_with("-setup.exe")) {
                    return Some(a.clone());
                }
                if let Some(a) = assets.iter().find(|a| a.name.to_lowercase().contains("arm64") && a.name.ends_with(".msi")) {
                    return Some(a.clone());
                }
            } else {
                if let Some(a) = assets.iter().find(|a| (a.name.to_lowercase().contains("x64") || a.name.to_lowercase().contains("x86_64")) && a.name.ends_with("-setup.exe")) {
                    return Some(a.clone());
                }
                if let Some(a) = assets.iter().find(|a| (a.name.to_lowercase().contains("x64") || a.name.to_lowercase().contains("x86_64")) && a.name.ends_with(".msi")) {
                    return Some(a.clone());
                }
            }
            // Exact names first: the Windows workflow publishes these two on
            // the `latest` release, and matching them by stem avoids picking
            // `kadr-portable.exe` (a bare exe that would install nothing).
            if let Some(a) = assets.iter().find(|a| a.name.eq_ignore_ascii_case("kadr-setup.exe")) {
                return Some(a.clone());
            }
            if let Some(a) = assets.iter().find(|a| a.name.ends_with("-setup.exe")) {
                return Some(a.clone());
            }
            if let Some(a) = assets.iter().find(|a| a.name.ends_with(".msi")) {
                return Some(a.clone());
            }
            if let Some(a) = assets
                .iter()
                .find(|a| a.name.ends_with(".exe") && !a.name.contains("portable"))
            {
                return Some(a.clone());
            }
        }
        "macos" => {
            if let Some(a) = assets.iter().find(|a| a.name.to_lowercase().contains("universal") && a.name.ends_with(".dmg")) {
                return Some(a.clone());
            }
            if let Some(a) = assets.iter().find(|a| a.name.to_lowercase().contains(arch) && a.name.ends_with(".dmg")) {
                return Some(a.clone());
            }
            if let Some(a) = assets.iter().find(|a| a.name.ends_with(".dmg")) {
                return Some(a.clone());
            }
        }
        "linux" => {
            if let Some(a) = assets.iter().find(|a| a.name.ends_with(".AppImage")) {
                return Some(a.clone());
            }
            if let Some(a) = assets.iter().find(|a| a.name.ends_with(".deb")) {
                return Some(a.clone());
            }
            if let Some(a) = assets.iter().find(|a| a.name.ends_with(".rpm")) {
                return Some(a.clone());
            }
        }
        _ => {}
    }
    None
}

static STATUS: OnceLock<Mutex<UpdateStatus>> = OnceLock::new();
static DOWNLOADING: AtomicBool = AtomicBool::new(false);

fn status_slot() -> &'static Mutex<UpdateStatus> {
    STATUS.get_or_init(|| Mutex::new(UpdateStatus::default()))
}

pub fn get_status() -> UpdateStatus {
    status_slot().lock().unwrap().clone()
}

/// Every network call the updater makes is bounded. Without a timeout a
/// stalled connection left the download flag set forever, and every later
/// attempt was refused as "busy" until the process restarted.
const METADATA_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// Browser URL of the release's `SHA256SUMS` asset, captured during the
/// metadata check. `None` when the release does not publish one.
static SUMS_URL: Mutex<Option<String>> = Mutex::new(None);

/// Where the update check reads the answer.
///
/// No GitHub **API** here on purpose. The API allows 60 requests per hour per
/// IP address without a token, and a desktop app that checks for updates was
/// answered with `403 rate limit exceeded` -- the check could not work even
/// though the release existed. These are plain release-asset URLs: a CDN file,
/// no rate limit, no login, no token to embed in the binary.
const DOWNLOAD_BASE: &str = "https://github.com/AbobaCDA/kadr/releases/download/latest";
const VERSION_URL: &str = concat!(
    "https://github.com/AbobaCDA/kadr/releases/download/latest/",
    "version.json"
);
const SETUP_URL: &str = concat!(
    "https://github.com/AbobaCDA/kadr/releases/download/latest/",
    "kadr-setup.exe"
);
/// The portable build: the application itself, no installer wrapper.
const PORTABLE_URL: &str = concat!(
    "https://github.com/AbobaCDA/kadr/releases/download/latest/",
    "kadr-portable.exe"
);
const PORTABLE_NAME: &str = "kadr-portable.exe";
const SUMS_FALLBACK_URL: &str = concat!(
    "https://github.com/AbobaCDA/kadr/releases/download/latest/",
    "SHA256SUMS"
);

/// URL of a release asset whose contents change while its name does not.
///
/// `version.json` and `SHA256SUMS` are replaced in place on every release, and
/// GitHub's asset CDN caches them for several minutes while quoting that
/// lifetime to browsers. A build published a minute ago could therefore be
/// answered with the *previous* file: the release carried 1.0.24 and the check
/// still read 1.0.23. A query string makes every check a URL the CDN has never
/// seen, so it goes to the origin. Not a secret, just a cache-buster.
fn fresh_asset_url(base: &str) -> String {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{base}?t={stamp}")
}

/// The release page, for "View on GitHub" and for the notes.
const RELEASE_PAGE: &str = "https://github.com/AbobaCDA/kadr/releases/tag/latest";

/// What the rolling `latest` release publishes about itself.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PublishedVersion {
    version: String,
    #[serde(default)]
    commit: String,
    #[serde(default)]
    built: String,
}

/// What the user is told when nothing newer exists.
///
/// GitHub's own words: the rolling tag is a prerelease, and `/releases/latest`
/// means "latest non-prerelease", which answered `404` for this repository.
/// Nothing here is a secret.
const ROLLING_TAG_NOTE: &str = "Filled in from the release page; this build reads `version.json` instead of the GitHub API.";

/// Reads `version.json` from the rolling release.
///
/// The one URL in the updater that changes between builds. GitHub serves
/// release assets through a CDN that caches them for a few minutes *and* quotes
/// that lifetime to ordinary browsers, so a build published two minutes ago can
/// still be answered with the previous answer. That bit once already: the
/// release carried 1.0.24 while `/latest/version.json` still said 1.0.23, and
/// the update check happily reported "you are up to date".
///
/// Passing the running version as a query string makes each build a different
/// URL, which the CDN has no copy of, so the first check after a release always
/// asks the origin. It is a cache-buster, not a secret: the file is public and
/// its contents are as well.
fn fetch_published_version(current_version: &str) -> Result<PublishedVersion, String> {
    let url = fresh_asset_url(VERSION_URL);
    let published: PublishedVersion = ureq::get(&url)
        .set("User-Agent", &format!("kadr-updater/{}", current_version))
        .set("Cache-Control", "no-cache")
        .timeout(METADATA_TIMEOUT)
        .call()
        .map_err(|e| format!("Failed to reach the release page: {e}"))?
        .into_json()
        .map_err(|e| format!("Failed to read version.json: {e}"))?;

    if published.version.trim().is_empty() {
        return Err("The release page published no version".to_string());
    }
    Ok(published)
}

/// Checks the rolling `latest` release for a newer build.
pub fn check_for_update() -> Result<UpdateInfo, String> {
    let current_version = env!("CARGO_PKG_VERSION");

    let published = fetch_published_version(current_version)?;

    // Checksums sit next to the installers; the download verifies against them
    // when present, exactly as it did with the API's asset list.
    if let Ok(mut slot) = SUMS_URL.lock() {
        *slot = Some(SUMS_FALLBACK_URL.to_string());
    }

    let latest_tag = published.version.trim().trim_start_matches('v').to_string();
    let has_update = is_newer_version(&latest_tag, current_version);
    let asset_name = "kadr-setup.exe".to_string();

    Ok(UpdateInfo {
        current_version: current_version.to_string(),
        latest_version: latest_tag.clone(),
        has_update,
        release_name: format!("Kadr {latest_tag}"),
        release_notes: format!(
            "Built {} from commit {}.\n\n{ROLLING_TAG_NOTE}",
            if published.built.is_empty() { "at an unknown time" } else { &published.built },
            if published.commit.is_empty() { "unknown" } else { &published.commit[..published.commit.len().min(7)] }
        ),
        release_url: RELEASE_PAGE.to_string(),
        published_at: published.built.clone(),
        asset_size: None,
        asset_url: Some(format!("{DOWNLOAD_BASE}/{asset_name}")),
        asset_name: Some(asset_name),
    })
}

#[cfg(test)]
mod fresh_url_tests {
    use super::{SUMS_FALLBACK_URL, VERSION_URL, fresh_asset_url};

    #[test]
    fn cache_buster_adds_a_query_and_keeps_the_path() {
        for base in [VERSION_URL, SUMS_FALLBACK_URL] {
            let url = fresh_asset_url(base);
            assert!(url.starts_with(base), "path must not change: {url}");
            assert!(url.contains("?t="), "a version.json left uncached is a stale one: {url}");
            let stamp = url.split("?t=").nth(1).expect("stamp");
            assert!(stamp.parse::<u64>().is_ok(), "stamp must be numeric: {stamp}");
        }
    }
}

/// True when this install is the portable build (a `portable` marker file
/// sits next to the executable).
///
/// A portable install must not run the installer: it would place a second
/// copy in Program Files while the portable folder - and the index inside it -
/// stayed exactly as it was, so the user is told they updated and nothing
/// they are running actually changed.
pub fn is_portable_install() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(dir) = exe.parent() else {
        return false;
    };
    dir.join("portable").is_file() || dir.join("kensaku.portable").is_file()
}

/// Expected SHA-256 for `asset_name` from the release's `SHA256SUMS`.
///
/// Returns `Ok(None)` when the release publishes no checksum file or does not
/// list this asset: an older tag must still be installable.
fn expected_sha256(asset_name: &str) -> Result<Option<String>, String> {
    let url = match SUMS_URL.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => None,
    };
    let Some(url) = url else {
        return Ok(None);
    };

    // The checksums are rewritten on every release, so they are read the same
    // way: a URL the CDN has not cached, not the plain tag path.
    let body = ureq::get(&fresh_asset_url(&url))
        .set(
            "User-Agent",
            &format!("kensaku-updater/{}", env!("CARGO_PKG_VERSION")),
        )
        .set("Cache-Control", "no-cache")
        .timeout(METADATA_TIMEOUT)
        .call()
        .map_err(|e| format!("Could not fetch checksums: {e}"))?
        .into_string()
        .map_err(|e| format!("Could not read checksums: {e}"))?;

    for line in body.lines() {
        // "<sha256>  <filename>" (binary mode) or "<sha256> <filename>".
        let mut parts = line.split_whitespace();
        let (Some(hash), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        let name = name.trim_start_matches('*');
        if name == asset_name {
            return Ok(Some(hash.to_ascii_lowercase()));
        }
    }
    Ok(None)
}

fn sha256_of(path: &std::path::Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path).map_err(|e| format!("Could not open download: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buf)
            .map_err(|e| format!("Could not read download: {e}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for &byte in digest.as_slice() {
        use std::fmt::Write as _;
        let _ = write!(hex, "{:02x}", byte);
    }
    Ok(hex)
}

/// Initiates download of the chosen asset in a background worker thread.
pub fn download_update(asset_url: &str, asset_name: &str) -> Result<UpdateStatus, String> {
    if is_portable_install() {
        return Err(
            "This is a portable install: update by replacing the folder with the new portable zip. \
             In-app updates are disabled so the running copy is never silently replaced by a \
             second install."
                .to_string(),
        );
    }

    // An install the user owns can be replaced in place, and then the right
    // download is the portable build: it *is* the application, with no
    // installer to run. See `self_update` for how the swap is carried out.
    let in_place = crate::self_update::can_swap_in_place();
    let (asset_url, asset_name) = if in_place {
        (PORTABLE_URL, PORTABLE_NAME)
    } else {
        (asset_url, asset_name)
    };

    if DOWNLOADING.swap(true, Ordering::SeqCst) {
        return Ok(get_status());
    }

    let update_dir = std::env::temp_dir().join("kensaku-updates");
    if let Err(e) = std::fs::create_dir_all(&update_dir) {
        DOWNLOADING.store(false, Ordering::SeqCst);
        let mut s = status_slot().lock().unwrap();
        s.busy = false;
        s.error = Some(format!("Could not create update directory: {e}"));
        return Ok(s.clone());
    }

    let final_path = update_dir.join(asset_name);
    let part_path = update_dir.join(format!("{}.part", asset_name));

    // Reset status
    {
        let mut s = status_slot().lock().unwrap();
        s.busy = true;
        s.error = None;
        s.ready_to_install = false;
        s.downloaded_path = None;
        s.progress = Some(UpdateProgress {
            phase: "downloading".to_string(),
            downloaded: 0,
            total: 0,
            percent: 0,
        });
    }

    let url = asset_url.to_string();
    let asset_name_for_check = asset_name.to_string();

    std::thread::Builder::new()
        .name("kensaku-updater".to_string())
        .spawn(move || {
            let res = (|| -> Result<(), String> {
                // Retries. A release asset is served from a CDN, and that CDN
                // answered 504 often enough to kill the workflow's own bundling
                // step -- a 20 MB download that dies at 90% should not have to
                // be restarted by hand. Each attempt truncates the `.part` file
                // it writes, so a retry never appends to a broken prefix.
                let mut last_error = String::new();
                let mut resp_opt = None;
                for attempt in 1..=3u32 {
                    match ureq::get(&url)
                        .set("User-Agent", &format!("kensaku-updater/{}", env!("CARGO_PKG_VERSION")))
                        .timeout(DOWNLOAD_TIMEOUT)
                        .call()
                    {
                        Ok(resp) => {
                            resp_opt = Some(resp);
                            break;
                        }
                        Err(e) => {
                            last_error = format!("Download request failed: {e}");
                            if attempt < 3 {
                                std::thread::sleep(std::time::Duration::from_secs(2));
                            }
                        }
                    }
                }
                let resp = resp_opt.ok_or(last_error)?;

                let total_bytes: u64 = resp
                    .header("Content-Length")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);

                let mut reader = resp.into_reader();
                let mut file = std::fs::File::create(&part_path)
                    .map_err(|e| format!("Failed to create temporary file: {e}"))?;

                let mut downloaded: u64 = 0;
                let mut buf = [0u8; 64 * 1024];

                loop {
                    use std::io::Read;
                    use std::io::Write;
                    let n = reader.read(&mut buf).map_err(|e| format!("Read error: {e}"))?;
                    if n == 0 {
                        break;
                    }
                    file.write_all(&buf[..n]).map_err(|e| format!("Write error: {e}"))?;
                    downloaded += n as u64;

                    let percent = if total_bytes > 0 {
                        ((downloaded as f64 / total_bytes as f64) * 100.0).min(100.0) as u8
                    } else {
                        0
                    };

                    let mut s = status_slot().lock().unwrap();
                    s.progress = Some(UpdateProgress {
                        phase: "downloading".to_string(),
                        downloaded,
                        total: total_bytes,
                        percent,
                    });
                }

                // A body that ends early reads as a clean EOF, so the loop
                // above cannot tell a finished download from a dropped
                // connection. Renaming the .part file unconditionally marked
                // a truncated installer as ready and the user ran it.
                if total_bytes > 0 && downloaded != total_bytes {
                    let _ = std::fs::remove_file(&part_path);
                    return Err(format!(
                        "Download incomplete: got {downloaded} of {total_bytes} bytes"
                    ));
                }

                // Verify against the release's published checksum before the
                // file is ever executable. The workflow already publishes
                // SHA256SUMS; the updater used to ignore it entirely.
                if let Some(expected) = expected_sha256(&asset_name_for_check)? {
                    let actual = sha256_of(&part_path)?;
                    if actual != expected {
                        let _ = std::fs::remove_file(&part_path);
                        return Err(format!(
                            "Update failed verification: expected {expected}, got {actual}"
                        ));
                    }
                }

                let _ = std::fs::remove_file(&final_path);
                std::fs::rename(&part_path, &final_path)
                    .map_err(|e| format!("Failed to finalize update file: {e}"))?;

                Ok(())
            })();

            let mut s = status_slot().lock().unwrap();
            s.busy = false;
            DOWNLOADING.store(false, Ordering::SeqCst);
            match res {
                Ok(()) => {
                    s.ready_to_install = true;
                    s.downloaded_path = Some(final_path.to_string_lossy().to_string());
                    if let Some(ref mut p) = s.progress {
                        p.phase = "ready".to_string();
                        p.percent = 100;
                    }
                }
                Err(err) => {
                    s.error = Some(err);
                    if let Some(ref mut p) = s.progress {
                        p.phase = "error".to_string();
                    }
                }
            }
        })
        .map_err(|e| format!("Failed to spawn updater thread: {e}"))?;

    Ok(get_status())
}

/// Spawns the installer detached and exits Kadr so files are replaced cleanly.
pub fn install_and_restart(app: &AppHandle) -> Result<(), String> {
    let path_str = {
        let s = status_slot().lock().unwrap();
        s.downloaded_path.clone().ok_or_else(|| "No update has been downloaded yet".to_string())?
    };

    let path = PathBuf::from(&path_str);
    if !path.is_file() {
        return Err(format!("Downloaded update file does not exist at {:?}", path));
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        const DETACHED_PROCESS: u32 = 0x00000008;

        // Preferred path: swap the executable, restart, done. No installer, no
        // UAC, no second copy of the app -- which is the whole complaint about
        // "updating" by re-installing.
        if crate::self_update::can_swap_in_place() {
            match crate::self_update::spawn_swap_helper(&path) {
                Ok(_) => {
                    crate::say_err!("[Kadr] update: swapping the executable in place");
                    app.exit(0);
                    return Ok(());
                }
                Err(e) => crate::say_err!(
                    "[Kadr] in-place update unavailable ({e}); running the installer instead"
                ),
            }
        }

        if path_str.ends_with(".msi") {
            let mut cmd = std::process::Command::new("msiexec");
            cmd.args(["/i", &path_str]);
            cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
            cmd.spawn().map_err(|e| format!("Failed to launch MSI installer: {e}"))?;
        } else {
            let mut cmd = std::process::Command::new(&path);
            cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
            cmd.spawn().map_err(|e| format!("Failed to launch installer: {e}"))?;
        }
        app.exit(0);
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        let _ = app;
        std::process::Command::new("open")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("Failed to open DMG installer: {e}"))?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        if path_str.ends_with(".AppImage") {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).map_err(|e| e.to_string())?.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(&path, perms);
            std::process::Command::new(&path).spawn().map_err(|e| format!("Failed to launch AppImage: {e}"))?;
            app.exit(0);
            Ok(())
        } else {
            let _ = app;
            std::process::Command::new("xdg-open")
                .arg(&path)
                .spawn()
                .map_err(|e| format!("Failed to launch package installer: {e}"))?;
            Ok(())
        }
    }

    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = app;
        Err("Unsupported operating system for in-app update launch".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer_version() {
        assert!(is_newer_version("1.0.14", "1.0.13"));
        assert!(is_newer_version("v1.0.14", "1.0.13"));
        assert!(is_newer_version("1.0.14-beta.1", "1.0.13"));
        assert!(is_newer_version("1.1.0", "1.0.13"));
        assert!(is_newer_version("2.0.0", "1.0.13"));

        assert!(!is_newer_version("1.0.13", "1.0.13"));
        assert!(!is_newer_version("v1.0.13", "1.0.13"));
        assert!(!is_newer_version("1.0.12", "1.0.13"));
        assert!(!is_newer_version("0.9.9", "1.0.13"));
    }

    #[test]
    fn the_version_file_drives_the_update_check() {
        // What the workflow publishes next to the installer. The check reads
        // this instead of the GitHub API, which answered 403 (60 requests per
        // hour per IP without a token).
        let payload = r#"{"version":"1.0.19","commit":"46d5bc8abc","built":"2026-11-20T08:00:00Z"}"#;
        let published: PublishedVersion = serde_json::from_str(payload).expect("payload");
        assert_eq!(published.version, "1.0.19");
        // The fields the UI shows must survive a minimal file too.
        let minimal: PublishedVersion =
            serde_json::from_str(r#"{"version":"1.0.20"}"#).expect("minimal payload");
        assert_eq!(minimal.commit, "");
    }

    #[test]
    fn the_update_urls_need_no_token_and_no_api() {
        // A plain release asset: CDN, no rate limit, no login. Guarding this
        // matters because the old code went through api.github.com, where an
        // anonymous client is capped at 60 requests per hour.
        for url in [VERSION_URL, SETUP_URL, SUMS_FALLBACK_URL, RELEASE_PAGE] {
            assert!(
                !url.contains("api.github.com"),
                "{url} must not depend on the rate-limited API"
            );
            assert!(url.starts_with("https://"), "{url} must be https");
        }
        assert!(VERSION_URL.ends_with("/version.json"));
        assert!(SETUP_URL.ends_with("/kadr-setup.exe"));
    }

    #[test]
    fn stable_release_names_win_over_versioned_and_portable_assets() {
        // What the `latest` release actually publishes: two fixed names plus a
        // checksum file. Picking `kadr-portable.exe` here would download a bare
        // exe that installs nothing, and picking a versioned setup file would
        // keep working only by accident.
        let assets = vec![
            ReleaseAsset {
                name: "kadr-portable.exe".to_string(),
                browser_download_url: "http://example.com/p.exe".to_string(),
                size: 10,
            },
            ReleaseAsset {
                name: "SHA256SUMS".to_string(),
                browser_download_url: "http://example.com/sums".to_string(),
                size: 1,
            },
            ReleaseAsset {
                name: "kadr-setup.exe".to_string(),
                browser_download_url: "http://example.com/s.exe".to_string(),
                size: 20,
            },
        ];
        if cfg!(windows) {
            let selected = select_platform_asset(&assets).unwrap();
            assert_eq!(selected.name, "kadr-setup.exe");
        }

        // With only a portable build present, never hand the installer path a
        // portable exe: `None` is the honest answer, and the UI says so.
        let portable_only = vec![ReleaseAsset {
            name: "kadr-portable.exe".to_string(),
            browser_download_url: "http://example.com/p.exe".to_string(),
            size: 10,
        }];
        if cfg!(windows) {
            assert!(select_platform_asset(&portable_only).is_none());
        }
    }

    #[test]
    fn test_select_platform_asset() {
        let assets = vec![
            ReleaseAsset {
                name: "kensaku_1.0.14_x64-setup.exe".to_string(),
                browser_download_url: "http://example.com/setup.exe".to_string(),
                size: 1000,
            },
            ReleaseAsset {
                name: "kensaku_1.0.14_arm64-setup.exe".to_string(),
                browser_download_url: "http://example.com/arm-setup.exe".to_string(),
                size: 1000,
            },
            ReleaseAsset {
                name: "kensaku_1.0.14_universal.dmg".to_string(),
                browser_download_url: "http://example.com/kensaku.dmg".to_string(),
                size: 1000,
            },
            ReleaseAsset {
                name: "kensaku_1.0.14_amd64.AppImage".to_string(),
                browser_download_url: "http://example.com/kensaku.AppImage".to_string(),
                size: 1000,
            },
        ];

        let selected = select_platform_asset(&assets);
        assert!(selected.is_some());
        let name = selected.unwrap().name;
        if cfg!(windows) {
            if cfg!(target_arch = "aarch64") {
                assert!(name.contains("arm64"));
            } else {
                assert!(name.contains("x64"));
            }
        } else if cfg!(target_os = "macos") {
            assert!(name.ends_with(".dmg"));
        } else if cfg!(target_os = "linux") {
            assert!(name.ends_with(".AppImage"));
        }
    }
}
