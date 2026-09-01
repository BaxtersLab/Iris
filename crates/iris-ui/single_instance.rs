// SPDX-License-Identifier: MIT
// Iris — iris-ui

//! One Iris at a time.
//!
//! Two instances cannot usefully coexist: they contend for the same camera —
//! V4L2 hands `/dev/videoN` to the first opener and refuses the second — and
//! for the same metrics port. Before 2026-08-31 the second instance did not
//! merely misbehave, it **aborted the process**: hyper's `Server::bind` panics
//! when the address is taken and the release profile is `panic = "abort"`, so
//! launching Iris twice produced a core dump and an Ubuntu crash report.
//!
//! Making the bind non-fatal stopped the crash but left the real problem: a
//! second Iris would run, fight the first for the camera, and show an empty
//! preview. So the second instance is now refused outright, which is what the
//! operator asked for and what the app actually wants.
//!
//! **Mechanism: `flock(2)` on a lock file, not a PID file.** A PID file has to
//! be cleaned up, and a process killed with SIGKILL never gets to do it — the
//! stale file then blocks every future start. An advisory `flock` is released
//! by the kernel when the file descriptor closes, which happens on exit however
//! the process dies, so there is no stale-lock state to recover from.

use std::path::PathBuf;

/// The result of trying to become the one running Iris.
#[derive(Debug)]
pub enum Instance {
    /// This process holds the lock. Keep the value alive for the whole run —
    /// dropping it releases the lock.
    Acquired(InstanceLock),
    /// Another Iris holds the lock. Its pid, if the lock file could be read.
    AlreadyRunning { pid: Option<i32>, path: PathBuf },
    /// The lock could not be evaluated (no writable runtime directory, for
    /// instance). **Startup continues.** Refusing to run because a lock could
    /// not be taken would make Iris unstartable in environments where the
    /// guard is merely unavailable, which is a worse failure than the one it
    /// prevents.
    Unavailable(std::io::Error),
}

/// Holds the lock open. The lock lives as long as this value.
#[derive(Debug)]
pub struct InstanceLock {
    /// unix: the flock`ed file. The lock lives exactly as long as this.
    #[cfg(unix)]
    _file: std::fs::File,
    /// windows: the named mutex. Same contract — Windows releases it when the
    /// process exits, including on a hard kill, which is why a mutex was
    /// chosen over a pid file for the same reason `flock` was on unix.
    #[cfg(windows)]
    _handle: MutexHandle,
    pub path: PathBuf,
}

/// Owns the named mutex and closes it on drop.
#[cfg(windows)]
#[derive(Debug)]
pub struct MutexHandle(pub(crate) isize);

#[cfg(windows)]
impl Drop for MutexHandle {
    fn drop(&mut self) {
        // Closing the handle releases the mutex. The OS would do this at
        // process exit anyway; doing it here keeps the lifetime explicit.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// Where the lock file lives, with the environment passed in rather than read.
///
/// `$XDG_RUNTIME_DIR` is the correct home for it — it is per-user, on tmpfs,
/// and cleared at logout. The `/tmp` fallback is per-uid so two users on one
/// machine do not lock each other out of their own sessions.
pub fn lock_path(runtime_dir: Option<String>, uid: u32) -> PathBuf {
    match runtime_dir.filter(|d| !d.is_empty() && std::path::Path::new(d).is_absolute()) {
        Some(dir) => PathBuf::from(dir).join("iris.lock"),
        None => PathBuf::from(format!("/tmp/iris-{uid}.lock")),
    }
}

#[cfg(unix)]
fn default_lock_path() -> PathBuf {
    lock_path(std::env::var("XDG_RUNTIME_DIR").ok(), unsafe { libc::getuid() })
}

/// Try to become the single running instance.
#[cfg(unix)]
pub fn acquire() -> Instance {
    use std::io::{Read, Seek, Write};
    use std::os::unix::io::AsRawFd;

    let path = default_lock_path();
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) => return Instance::Unavailable(e),
    };

    // LOCK_NB: fail immediately rather than wait for the other instance to exit.
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if locked != 0 {
        let err = std::io::Error::last_os_error();
        // EWOULDBLOCK is "someone else holds it" — the case we are here for.
        // Anything else means the lock could not be evaluated at all.
        if err.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Instance::Unavailable(err);
        }
        let mut buf = String::new();
        let pid = file
            .read_to_string(&mut buf)
            .ok()
            .and_then(|_| buf.trim().parse::<i32>().ok());
        return Instance::AlreadyRunning { pid, path };
    }

    // Record our pid so the next launcher can name who is holding it. Best
    // effort: the lock is already ours, and a failure to write the pid must not
    // give it up.
    let _ = file.set_len(0);
    let _ = file.rewind();
    let _ = write!(file, "{}", std::process::id());
    let _ = file.flush();

    Instance::Acquired(InstanceLock { _file: file, path })
}


/// The Windows name for the single-instance mutex.
///
/// `Local\` scopes it to the login session, which is what we want: two
/// different users signed into the same machine each get their own Iris, the
/// same way the unix `/tmp` fallback is per-uid. A `Global\` name would let one
/// user's Iris block another's.
#[cfg(windows)]
pub const MUTEX_NAME: &str = r"Local\BaxtersLab.Iris.SingleInstance";

/// Windows single-instance guard: a named mutex.
///
/// Chosen for the same property that made `flock` right on unix — **the kernel
/// releases it however the process dies.** A pid file left behind by a hard
/// kill blocks every future start; a mutex handle is closed by the OS at
/// process teardown, crash included, so there is no stale state to clean up.
///
/// `CreateMutexW` succeeds either way; the discriminator is
/// `ERROR_ALREADY_EXISTS` from `GetLastError`, which means someone else created
/// it first. In that case the handle we just received is closed immediately —
/// keeping it would hold a second reference to a mutex we do not own.
#[cfg(windows)]
pub fn acquire() -> Instance {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let wide: Vec<u16> = MUTEX_NAME.encode_utf16().chain(std::iter::once(0)).collect();
    // bInitialOwner = false: ownership is irrelevant here. Existence is the
    // signal, and not owning it avoids the abandoned-mutex state entirely.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
    if handle == 0 {
        return Instance::Unavailable(std::io::Error::last_os_error());
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // Another Iris created it. Close our reference and report it.
        unsafe { CloseHandle(handle) };
        return Instance::AlreadyRunning {
            // A named mutex carries no payload, so there is no pid to report.
            // `main.rs` already handles `None`.
            pid: None,
            path: PathBuf::from(MUTEX_NAME),
        };
    }
    Instance::Acquired(InstanceLock {
        _handle: MutexHandle(handle),
        path: PathBuf::from(MUTEX_NAME),
    })
}
#[cfg(all(not(unix), not(windows)))]
pub fn acquire() -> Instance {
    // Windows needs a named mutex (CreateMutexW + ERROR_ALREADY_EXISTS) rather
    // than flock. Not implemented here because this box cannot build or run the
    // Windows target; declared in ROADMAP.md.
    Instance::Unavailable(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "single-instance guard is not implemented on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runtime_dir_is_preferred() {
        assert_eq!(
            lock_path(Some("/run/user/1000".into()), 1000).to_str(),
            Some("/run/user/1000/iris.lock")
        );
    }

    /// Per-uid, so two users on one machine do not lock each other out.
    #[test]
    fn the_tmp_fallback_is_per_uid() {
        assert_eq!(lock_path(None, 1000).to_str(), Some("/tmp/iris-1000.lock"));
        assert_eq!(lock_path(None, 1001).to_str(), Some("/tmp/iris-1001.lock"));
    }

    /// An env var set to "" is set, and a relative value must be ignored — the
    /// same XDG rule the config search uses. Either would put the lock
    /// somewhere that depends on the working directory.
    #[test]
    fn empty_and_relative_runtime_dirs_fall_back() {
        assert_eq!(lock_path(Some(String::new()), 7).to_str(), Some("/tmp/iris-7.lock"));
        assert_eq!(lock_path(Some("relative/dir".into()), 7).to_str(), Some("/tmp/iris-7.lock"));
    }

    /// The real behaviour: a second flock on the same file is refused, which
    /// is what makes a second Iris refuse to start. Uses its own file rather
    /// than the live lock so it cannot interfere with a running instance.
    #[cfg(unix)]
    #[test]
    fn a_second_lock_on_the_same_file_is_refused() {
        use std::os::unix::io::AsRawFd;
        let dir = std::env::temp_dir().join(format!("iris-lock-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("iris.lock");

        let first = std::fs::OpenOptions::new()
            .read(true).write(true).create(true).truncate(false)
            .open(&path).expect("open first");
        let a = unsafe { libc::flock(first.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(a, 0, "the first lock must succeed");

        let second = std::fs::OpenOptions::new()
            .read(true).write(true).create(true).truncate(false)
            .open(&path).expect("open second");
        let b = unsafe { libc::flock(second.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_ne!(b, 0, "the second lock must be refused while the first is held");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EWOULDBLOCK),
            "refusal must be EWOULDBLOCK — anything else is a different failure"
        );

        // Closing the first descriptor releases the lock: no cleanup needed,
        // which is the whole reason this is not a pid file.
        drop(first);
        let c = unsafe { libc::flock(second.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(c, 0, "the lock must be free once the holder's fd closes");

        drop(second);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Windows guard must actually refuse a second acquire.
    ///
    /// `CreateMutexW` reports `ERROR_ALREADY_EXISTS` regardless of which
    /// process created the mutex first, so holding one and asking again inside
    /// a single test exercises the real discriminator rather than a simulation
    /// of it.
    #[cfg(windows)]
    #[test]
    fn a_second_acquire_is_refused_while_the_first_is_held() {
        let first = super::acquire();
        let _held = match first {
            super::Instance::Acquired(lock) => lock,
            other => panic!("expected to acquire the mutex first, got {other:?}"),
        };
        match super::acquire() {
            super::Instance::AlreadyRunning { pid, path } => {
                assert!(pid.is_none(), "a named mutex carries no pid, got {pid:?}");
                assert_eq!(path, std::path::PathBuf::from(super::MUTEX_NAME));
            }
            other => panic!("second acquire must be refused, got {other:?}"),
        }
    }

    /// Releasing the handle must free the name again — this is the property
    /// that makes a hard kill recoverable, since Windows closes handles at
    /// process teardown.
    #[cfg(windows)]
    #[test]
    fn releasing_the_handle_frees_the_name() {
        match super::acquire() {
            super::Instance::Acquired(lock) => drop(lock),
            other => panic!("expected to acquire, got {other:?}"),
        }
        match super::acquire() {
            super::Instance::Acquired(_) => {}
            other => panic!("the name must be free after the handle is dropped, got {other:?}"),
        }
    }

    /// `Local\` scopes the mutex to the login session. `Global\` would let one
    /// signed-in user's Iris block another's, which the per-uid `/tmp`
    /// fallback on unix deliberately avoids.
    #[cfg(windows)]
    #[test]
    fn the_mutex_is_session_scoped_not_machine_wide() {
        assert!(
            super::MUTEX_NAME.starts_with(r"Local\"),
            "expected a Local-scoped name, got {}",
            super::MUTEX_NAME
        );
        assert!(!super::MUTEX_NAME.starts_with(r"Global\"));
    }
}
