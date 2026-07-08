use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

#[cfg(unix)]
pub fn create_dir_link(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
pub fn create_dir_link(target: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

pub fn is_dir_link(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

pub fn read_dir_link(path: &Path) -> io::Result<PathBuf> {
    fs::read_link(path)
}

#[cfg(unix)]
pub fn remove_dir_link(path: &Path) -> io::Result<()> {
    fs::remove_file(path)
}

#[cfg(windows)]
pub fn remove_dir_link(path: &Path) -> io::Result<()> {
    fs::remove_dir(path)
}

#[cfg(unix)]
pub fn pid_is_alive(pid: u32) -> bool {
    // Linux/WSL best-effort active protection. A future version can compare
    // process start time from /proc/<pid>/stat to reduce PID reuse ambiguity.
    Path::new("/proc").join(pid.to_string()).exists()
}

#[cfg(windows)]
pub fn pid_is_alive(pid: u32) -> bool {
    // Best-effort and conservative. If tasklist itself fails, report alive so
    // clean/gc keep protecting the layer rather than deleting a live install.
    let filter = format!("PID eq {pid}");
    let output = Command::new("tasklist")
        .args(["/FI", &filter, "/NH"])
        .output();
    let Ok(output) = output else {
        return true;
    };
    if !output.status.success() {
        return true;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .split_whitespace()
        .any(|part| part.trim() == pid.to_string())
}

#[cfg(unix)]
pub fn default_rim_base() -> Result<PathBuf, String> {
    match env::var("RIM_PROFILE").ok().as_deref() {
        None | Some("") | Some("ram") => Ok(PathBuf::from("/dev/shm/rim")),
        Some("cache") => Ok(cache_dir().join("rim")),
        Some("external") => env::var_os("RIM_EXTERNAL_BASE")
            .map(PathBuf::from)
            .ok_or_else(|| "RIM_PROFILE=external requires RIM_EXTERNAL_BASE".to_owned()),
        Some(other) => Err(format!(
            "unknown RIM_PROFILE={other}; expected ram, cache, or external"
        )),
    }
}

#[cfg(windows)]
pub fn default_rim_base() -> Result<PathBuf, String> {
    match env::var("RIM_PROFILE").ok().as_deref() {
        None | Some("") | Some("cache") => local_app_data().map(|path| path.join("rim")),
        Some("external") => env::var_os("RIM_EXTERNAL_BASE")
            .map(PathBuf::from)
            .ok_or_else(|| "RIM_PROFILE=external requires RIM_EXTERNAL_BASE; rim will not guess a drive like D:\\rim".to_owned()),
        Some("ram") => Err(
            "RIM_PROFILE=ram is not available on native Windows. Use WSL for /dev/shm tmpfs behavior, or set RIM_BASE to a RAM disk manually."
                .to_owned(),
        ),
        Some(other) => Err(format!(
            "unknown RIM_PROFILE={other}; expected cache, external, or ram"
        )),
    }
}

#[cfg(unix)]
fn cache_dir() -> PathBuf {
    env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"))
}

#[cfg(windows)]
fn local_app_data() -> Result<PathBuf, String> {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "LOCALAPPDATA is not set; set RIM_BASE explicitly".to_owned())
}

#[cfg(unix)]
pub fn platform_name() -> &'static str {
    "unix"
}

#[cfg(windows)]
pub fn platform_name() -> &'static str {
    "windows"
}

#[cfg(unix)]
pub fn configure_child_command(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            restore_default_signals();
            Ok(())
        });
    }
}

#[cfg(windows)]
pub fn configure_child_command(_command: &mut Command) {
    // MVP: no-op. Windows does not support Unix pre_exec signal restoration.
}

#[cfg(unix)]
pub fn exit_status_code(status: ExitStatus) -> u8 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .map(|code| code.clamp(0, 255) as u8)
        .or_else(|| {
            status
                .signal()
                .map(|signal| (128 + signal).clamp(0, 255) as u8)
        })
        .unwrap_or(1)
}

#[cfg(windows)]
pub fn exit_status_code(status: ExitStatus) -> u8 {
    status.code().unwrap_or(1).clamp(0, 255) as u8
}

#[cfg(unix)]
const SIGINT: i32 = 2;
#[cfg(unix)]
const SIGTERM: i32 = 15;
#[cfg(unix)]
const SIG_DFL: usize = 0;

#[cfg(unix)]
type SignalHandler = usize;

#[cfg(unix)]
unsafe extern "C" {
    fn signal(signum: i32, handler: SignalHandler) -> SignalHandler;
}

#[cfg(unix)]
extern "C" fn record_signal(_signal: i32) {}

pub struct SignalGuard {
    #[cfg(unix)]
    previous_int: SignalHandler,
    #[cfg(unix)]
    previous_term: SignalHandler,
}

impl SignalGuard {
    #[cfg(unix)]
    pub fn install() -> Self {
        let previous_int = unsafe { signal(SIGINT, record_signal as *const () as SignalHandler) };
        let previous_term = unsafe { signal(SIGTERM, record_signal as *const () as SignalHandler) };
        Self {
            previous_int,
            previous_term,
        }
    }

    #[cfg(windows)]
    pub fn install() -> Self {
        Self {}
    }
}

#[cfg(unix)]
impl Drop for SignalGuard {
    fn drop(&mut self) {
        unsafe {
            signal(SIGINT, self.previous_int);
            signal(SIGTERM, self.previous_term);
        }
    }
}

#[cfg(windows)]
impl Drop for SignalGuard {
    fn drop(&mut self) {}
}

#[cfg(unix)]
fn restore_default_signals() {
    unsafe {
        signal(SIGINT, SIG_DFL);
        signal(SIGTERM, SIG_DFL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_name_is_known() {
        assert!(matches!(platform_name(), "unix" | "windows"));
    }

    #[test]
    #[cfg(unix)]
    fn unix_default_ram_profile_uses_dev_shm() {
        // This test assumes the default test environment does not override RIM_BASE.
        // Integration tests cover env isolation more thoroughly.
        let old = env::var_os("RIM_PROFILE");
        unsafe {
            env::remove_var("RIM_PROFILE");
        }
        assert_eq!(default_rim_base().unwrap(), PathBuf::from("/dev/shm/rim"));
        unsafe {
            if let Some(old) = old {
                env::set_var("RIM_PROFILE", old);
            }
        }
    }

    #[test]
    #[cfg(windows)]
    fn windows_ram_profile_is_rejected() {
        let old = env::var_os("RIM_PROFILE");
        unsafe {
            env::set_var("RIM_PROFILE", "ram");
        }
        let err = default_rim_base().unwrap_err();
        assert!(err.contains("RIM_PROFILE=ram is not available on native Windows"));
        unsafe {
            if let Some(old) = old {
                env::set_var("RIM_PROFILE", old);
            } else {
                env::remove_var("RIM_PROFILE");
            }
        }
    }
    #[test]
    #[cfg(windows)]
    fn windows_cache_profile_uses_local_app_data() {
        let old_profile = env::var_os("RIM_PROFILE");
        let old_local = env::var_os("LOCALAPPDATA");
        unsafe {
            env::set_var("RIM_PROFILE", "cache");
            env::set_var("LOCALAPPDATA", r"C:\Users\rim\AppData\Local");
        }
        assert_eq!(
            default_rim_base().unwrap(),
            PathBuf::from(r"C:\Users\rim\AppData\Local").join("rim")
        );
        unsafe {
            match old_profile {
                Some(value) => env::set_var("RIM_PROFILE", value),
                None => env::remove_var("RIM_PROFILE"),
            }
            match old_local {
                Some(value) => env::set_var("LOCALAPPDATA", value),
                None => env::remove_var("LOCALAPPDATA"),
            }
        }
    }

    #[test]
    #[cfg(windows)]
    fn windows_external_profile_requires_explicit_base() {
        let old_profile = env::var_os("RIM_PROFILE");
        let old_external = env::var_os("RIM_EXTERNAL_BASE");
        unsafe {
            env::set_var("RIM_PROFILE", "external");
            env::remove_var("RIM_EXTERNAL_BASE");
        }
        let err = default_rim_base().unwrap_err();
        assert!(err.contains("RIM_PROFILE=external requires RIM_EXTERNAL_BASE"));
        unsafe {
            match old_profile {
                Some(value) => env::set_var("RIM_PROFILE", value),
                None => env::remove_var("RIM_PROFILE"),
            }
            match old_external {
                Some(value) => env::set_var("RIM_EXTERNAL_BASE", value),
                None => env::remove_var("RIM_EXTERNAL_BASE"),
            }
        }
    }
    fn test_temp_dir(prefix: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = env::temp_dir().join(format!("rim-platform-{prefix}-{n}"));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn dir_link_helper_round_trip_or_skips_permission_denied() {
        let root = test_temp_dir("dir-link");
        let target = root.join("target");
        let link = root.join("link");
        fs::create_dir_all(&target).expect("target dir");
        match create_dir_link(&target, &link) {
            Ok(()) => {}
            Err(err) if cfg!(windows) => {
                eprintln!(
                    "skipping Windows symlink round-trip test; symlink privilege unavailable: {err}"
                );
                let _ = fs::remove_dir_all(&root);
                return;
            }
            Err(err) => panic!("create_dir_link failed: {err}"),
        }
        assert!(is_dir_link(&link));
        assert_eq!(read_dir_link(&link).expect("read link"), target);
        remove_dir_link(&link).expect("remove link");
        assert!(!link.exists());
        let _ = fs::remove_dir_all(&root);
    }
}
