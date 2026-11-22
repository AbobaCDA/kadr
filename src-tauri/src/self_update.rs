//! Replacing the running executable, instead of re-installing the application.
//!
//! The in-app update used to hand the downloaded `kadr-setup.exe` to Windows and
//! exit: the user got an installer, a UAC prompt, a "next-next-finish" and a
//! second copy of the app on top of the first. All they asked for was "make it
//! the new version".
//!
//! A running `.exe` cannot be overwritten, but it *can* be renamed, and another
//! process can then put a new file in its place. So the update works like this:
//!
//! 1. Kadr downloads the portable build next to its data, not the installer.
//! 2. On "restart and update" it copies *itself* to a helper in the temp
//!    directory and starts that helper with the three arguments below.
//! 3. Kadr exits, the helper waits for the process to be gone, renames the new
//!    file over the old path (with retries: antivirus holds files open for a
//!    moment), and starts the app again.
//!
//! The helper is the same binary as the app, so the cost is one extra ~25 MB
//! copy in the temp folder, deleted after the swap.
//!
//! When this cannot work -- an install directory that needs administrator
//! rights, an exe that is not named `kadr.exe`, a missing download -- the
//! caller falls back to the installer exactly as before. Nothing here breaks
//! the old path, it just stops being the default.

use std::path::{Path, PathBuf};

/// First argument that turns this binary into the swap helper.
pub const SWAP_FLAG: &str = "--kadr-swap";
/// How long the helper waits for the old process to release the exe.
const WAIT_STEPS: u32 = 240; // 240 * 250 ms = 60 s
const STEP_MS: u64 = 250;

/// Arguments the helper needs, in order after [`SWAP_FLAG`].
pub struct SwapRequest {
    /// The freshly downloaded build.
    pub new_exe: PathBuf,
    /// Where the running build lives.
    pub target_exe: PathBuf,
    /// Process id to wait for.
    pub pid: u32,
}

impl SwapRequest {
    /// Parses `--kadr-swap <new> <target> <pid>`. `None` for a normal launch.
    pub fn from_args<I: IntoIterator<Item = String>>(args: I) -> Option<Self> {
        let mut args = args.into_iter();
        if args.next().as_deref() != Some(SWAP_FLAG) {
            return None;
        }
        let new_exe = PathBuf::from(args.next()?);
        let target_exe = PathBuf::from(args.next()?);
        let pid = args.next()?.parse().ok()?;
        Some(Self {
            new_exe,
            target_exe,
            pid,
        })
    }

    fn to_args(&self) -> [String; 4] {
        [
            SWAP_FLAG.to_string(),
            self.new_exe.to_string_lossy().to_string(),
            self.target_exe.to_string_lossy().to_string(),
            self.pid.to_string(),
        ]
    }
}

/// Path of the helper copy: the same build, in the temp folder.
fn helper_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    if exe.file_name()?.to_str()? != "kadr.exe" {
        // A dev build (`kensaku.exe`, `cadr`) or an unexpected name: leave it
        // alone and use the installer path.
        return None;
    }
    Some(std::env::temp_dir().join("kadr-swap.exe"))
}

/// True when the running build can be replaced in place.
///
/// The test is concrete, not guessed: try to create a file next to the exe. A
/// `Program Files` install answers "no" (the user is not an administrator), and
/// those users keep the installer flow, which is what a UAC prompt is for.
pub fn can_swap_in_place() -> bool {
    let Some(helper) = helper_path() else {
        return false;
    };
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(dir) = exe.parent() else {
        return false;
    };
    let probe = dir.join(".kadr-write-probe");
    match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            // A stale helper from a crashed attempt must not block the next one.
            let _ = std::fs::remove_file(&helper);
            true
        }
        Err(_) => false,
    }
}

/// Starts the helper that will replace the running exe after this process ends.
///
/// Returns the helper's process id; the caller exits immediately afterwards.
pub fn spawn_swap_helper(downloaded: &Path) -> Result<u32, String> {
    let helper = helper_path().ok_or_else(|| "this build cannot replace itself".to_string())?;
    let current = std::env::current_exe()
        .map_err(|e| format!("could not find the running executable: {e}"))?;

    std::fs::copy(&current, &helper)
        .map_err(|e| format!("could not stage the updater helper: {e}"))?;

    let request = SwapRequest {
        new_exe: downloaded.to_path_buf(),
        target_exe: current,
        pid: std::process::id(),
    };

    let mut cmd = std::process::Command::new(&helper);
    cmd.args(request.to_args());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let child = cmd
        .spawn()
        .map_err(|e| format!("could not start the updater helper: {e}"))?;
    Ok(child.id())
}

/// Runs the swap if this process was started as the helper. Never returns in
/// that case: the process exits after the new build is started.
pub fn run_helper_if_asked() {
    let request = match SwapRequest::from_args(std::env::args().skip(1)) {
        Some(request) => request,
        None => return,
    };
    let ok = perform_swap(&request);
    let _ = ok;
    cleanup_helper();
    std::process::exit(0);
}

/// Waits for the old process, replaces the exe, starts it again.
pub fn perform_swap(request: &SwapRequest) -> bool {
    wait_for_exit(request.pid);
    // Antivirus and the search indexer hold a just-written file open for a
    // moment; a single rename then fails with "access denied" and the user is
    // left with an app that did not update.
    for _ in 0..40 {
        if std::fs::rename(&request.new_exe, &request.target_exe).is_ok() {
            let _ = std::process::Command::new(&request.target_exe).spawn();
            return true;
        }
        // A rename across volumes (temp on C:, app on D:) fails outright; copy.
        if std::fs::copy(&request.new_exe, &request.target_exe).is_ok() {
            let _ = std::fs::remove_file(&request.new_exe);
            let _ = std::process::Command::new(&request.target_exe).spawn();
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(STEP_MS));
    }
    false
}

/// Waits (bounded) for a process to disappear.
#[cfg(windows)]
fn wait_for_exit(pid: u32) {
    use std::ffi::c_void;
    type Handle = *mut c_void;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const WAIT_OBJECT_0: u32 = 0;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn WaitForSingleObject(handle: Handle, millis: u32) -> u32;
        fn CloseHandle(handle: Handle) -> i32;
    }
    // SAFETY: OpenProcess/Wait/Close on a handle this call owns; every call is
    // checked for failure before its handle is used.
    unsafe {
        let handle = OpenProcess(SYNCHRONIZE, 0, pid);
        if !handle.is_null() {
            let code = WaitForSingleObject(handle, WAIT_STEPS * STEP_MS as u32);
            let _ = code == WAIT_OBJECT_0;
            CloseHandle(handle);
        }
    }
}

#[cfg(not(windows))]
fn wait_for_exit(pid: u32) {
    // `kill(pid, 0)` succeeds while the process is alive. Polling is enough
    // here: this path exists for completeness, the shipped target is Windows.
    for _ in 0..WAIT_STEPS {
        let alive = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false);
        if !alive {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(STEP_MS));
    }
}

fn cleanup_helper() {
    if let Some(helper) = helper_path()
        && std::env::current_exe().ok().as_deref() == Some(helper.as_path())
    {
        // The running helper image cannot delete itself; a copy in the temp dir
        // is cheap, and `can_swap_in_place` clears it before the next swap.
        let _ = std::fs::remove_file(&helper);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_normal_launch_is_not_a_swap() {
        let args = ["--flag", "value"].iter().map(|s| s.to_string());
        assert!(SwapRequest::from_args(args).is_none());
        assert!(SwapRequest::from_args(std::iter::empty()).is_none());
    }

    #[test]
    fn a_malformed_swap_request_is_refused() {
        // Half a request must not rename something arbitrary.
        let args = [SWAP_FLAG, "C:\\new.exe"].iter().map(|s| s.to_string());
        assert!(SwapRequest::from_args(args).is_none());
        let args = [SWAP_FLAG, "a", "b", "not-a-pid"]
            .iter()
            .map(|s| s.to_string());
        assert!(SwapRequest::from_args(args).is_none());
    }

    #[test]
    fn a_full_request_round_trips_through_argv() {
        let request = SwapRequest {
            new_exe: PathBuf::from("C:\\temp\\kadr-portable.exe"),
            target_exe: PathBuf::from("C:\\Apps\\kadr.exe"),
            pid: 4242,
        };
        let parsed = SwapRequest::from_args(request.to_args().into_iter()).expect("parses");
        assert_eq!(parsed.new_exe, request.new_exe);
        assert_eq!(parsed.target_exe, request.target_exe);
        assert_eq!(parsed.pid, 4242);
    }

    #[test]
    fn the_helper_never_targets_an_unexpected_binary_name() {
        // A dev build must not start renaming itself over itself.
        if std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .as_deref()
            != Some("kadr.exe")
        {
            assert!(!can_swap_in_place());
        }
    }
}
