// SPDX-License-Identifier: MIT
// Iris — iris-hal
//! An exclusive lock on the capture device, held **across processes**.
//!
//! # Why this exists
//!
//! One camera, several test binaries. `cargo test` runs each crate's tests in
//! its own **process**, so a `static Mutex` — which is what `iris-hal` and
//! `iris-capture` each had — serialises only the tests inside one binary. With
//! `IRIS_USE_HW=1 cargo test --workspace` those binaries run concurrently, both
//! open `/dev/video0`, and the second `VIDIOC_S_FMT` fails with
//! `Device or resource busy (os error 16)`.
//!
//! This is the third form of the same defect in this project: two Windows
//! single-instance tests sharing one mutex name (round 5, 12 failures in 20),
//! three V4L2 tests sharing one camera in `iris-hal`, then two more in
//! `iris-capture`. Each was fixed locally and the next appeared somewhere the
//! previous fix could not reach.
//!
//! # Mechanism
//!
//! `flock(2)` on a lock file, for the same reasons `single_instance` chose it:
//! the kernel releases the lock when the file descriptor closes, so a panicking
//! or killed test cannot wedge the lock, and no stale state survives a crash.
//!
//! **It also serialises within a single process**, which is what lets it replace
//! the `static Mutex` rather than sit beside it: `flock` associates a lock with
//! an *open file description*, and each guard here opens its own, so two threads
//! in one binary contend exactly as two processes do.
//!
//! The wait is **blocking** (`LOCK_EX`, not `LOCK_NB`). A test that has to wait
//! for the camera should wait; failing fast would only recreate the flakiness
//! this removes.

// Both are used only by the unix implementation; ungated they are dead imports
// on the windows cross-check, which this workspace keeps warning-free.
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::path::PathBuf;

/// Holds the device lock. Releasing happens on drop, or on process exit.
#[derive(Debug)]
pub struct DeviceLock {
    /// Kept solely to hold the lock: dropping the file releases the `flock`.
    #[cfg(unix)]
    _file: std::fs::File,
}

/// Where the lock file lives.
///
/// `$XDG_RUNTIME_DIR` when it is a usable absolute path — per-user, on tmpfs,
/// cleared at logout — and a per-uid `/tmp` path otherwise, so two users on one
/// machine do not block each other. The same reasoning as
/// `iris-ui`'s `single_instance::lock_path`, and deliberately a **different
/// file**: this guards the camera between test processes, not the application
/// against a second copy of itself. Sharing one file would make a running Iris
/// and a test suite fight over a lock that means two different things.
#[cfg(unix)]
pub fn lock_path(runtime_dir: Option<String>, uid: u32) -> PathBuf {
    match runtime_dir.filter(|d| !d.is_empty() && std::path::Path::new(d).is_absolute()) {
        Some(dir) => PathBuf::from(dir).join("iris-capture-device.lock"),
        None => PathBuf::from(format!("/tmp/iris-capture-device-{uid}.lock")),
    }
}

/// Take the capture device exclusively, waiting until it is free.
///
/// Call this at the top of any test that opens a real camera. The returned
/// guard must be bound (`let _dev = lock_capture_device();`) — binding it to
/// `_` alone would drop it immediately and lock nothing.
#[cfg(unix)]
pub fn lock_capture_device() -> DeviceLock {
    let path = lock_path(std::env::var("XDG_RUNTIME_DIR").ok(), unsafe { libc::getuid() });
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .unwrap_or_else(|e| panic!("cannot open the device lock at {}: {e}", path.display()));

    // Blocking, and EINTR-retried: a signal must not silently hand back an
    // unlocked guard, which would look like success and reintroduce the race.
    loop {
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if rc == 0 {
            break;
        }
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::Interrupted {
            panic!("flock on {} failed: {err}", path.display());
        }
    }
    DeviceLock { _file: file }
}

/// Non-unix: there is no `flock`, and the Windows suites serialise inside their
/// own binary with a `static Mutex`. Cross-process contention on Windows would
/// need `LockFileEx` and is **not** solved here — declared rather than pretended
/// away, because a silent no-op that looked like a lock would be worse than
/// none at all.
#[cfg(not(unix))]
pub fn lock_capture_device() -> DeviceLock {
    DeviceLock {}
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// `$XDG_RUNTIME_DIR` when usable, per-uid `/tmp` otherwise — so two users
    /// on one machine do not block each other out of their own cameras.
    #[test]
    fn the_lock_path_is_per_user() {
        assert_eq!(
            lock_path(Some("/run/user/1000".into()), 1000),
            PathBuf::from("/run/user/1000/iris-capture-device.lock")
        );
        assert_eq!(
            lock_path(None, 1000),
            PathBuf::from("/tmp/iris-capture-device-1000.lock")
        );
        // Empty or relative values are not usable directories.
        assert_eq!(lock_path(Some(String::new()), 7), PathBuf::from("/tmp/iris-capture-device-7.lock"));
        assert_eq!(lock_path(Some("relative/dir".into()), 7), PathBuf::from("/tmp/iris-capture-device-7.lock"));
    }

    /// **It must not collide with the application's single-instance lock.**
    /// They mean different things — "a test is using the camera" versus "Iris is
    /// already running" — and sharing one file would make a running Iris and a
    /// test suite block each other for no reason.
    #[test]
    fn it_is_a_different_file_from_the_single_instance_lock() {
        let p = lock_path(Some("/run/user/1000".into()), 1000);
        assert_ne!(p, PathBuf::from("/run/user/1000/iris.lock"));
        assert_ne!(lock_path(None, 1000), PathBuf::from("/tmp/iris-1000.lock"));
    }

    /// **The claim that let the `static Mutex`es be deleted**: `flock`
    /// serialises two holders *inside one process*, because each guard opens its
    /// own file description.
    ///
    /// If this were false, replacing the mutexes would have removed protection
    /// rather than widened it — the tests in one binary would race again.
    #[test]
    fn a_second_holder_in_the_same_process_waits() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        let held = Duration::from_millis(300);
        let (ready_tx, ready_rx) = mpsc::channel();

        let holder = std::thread::spawn(move || {
            let _lock = lock_capture_device();
            ready_tx.send(()).expect("signal");
            std::thread::sleep(held);
        });

        ready_rx.recv().expect("holder must acquire first");
        let t0 = Instant::now();
        let second = lock_capture_device();
        let waited = t0.elapsed();
        drop(second);
        holder.join().expect("holder thread");

        assert!(
            waited >= held / 2,
            "the second holder returned in {waited:?} without waiting — flock is \
             not serialising inside one process, so deleting the static Mutex \
             would have removed protection"
        );
    }
}
