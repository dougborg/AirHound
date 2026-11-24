//! Regression coverage for the test-only event witnesses, not runtime timing.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod support;
use std::{
    io,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ah-events-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
#[cfg_attr(coverage_nightly, coverage(off))]
fn replacement_after_parent_event_rearms_new_file_before_subsequent_write() {
    let root = Root::new();
    let path = root.0.join("database");
    let staged = root.0.join("staged");
    std::fs::write(&path, b"old").unwrap();
    let mut events = support::FileEvents::new(&[&path]).unwrap();
    let (release, ready) = std::sync::mpsc::channel();
    let writer_path = path.clone();
    let writer = std::thread::spawn(move || {
        ready.recv().unwrap();
        std::fs::write(writer_path, b"complete").unwrap();
    });
    let mut phase = 0;
    events
        .until(|| {
            match phase {
                0 => {
                    std::fs::write(&staged, b"new")?;
                    phase = 1;
                }
                1 => {
                    assert_eq!(std::fs::read(&path)?, b"old");
                    std::fs::rename(&staged, &path)?;
                    phase = 2;
                }
                2 => {
                    assert_eq!(std::fs::read(&path)?, b"new");
                    release.send(()).unwrap();
                    phase = 3;
                }
                _ => {
                    if std::fs::read(&path)? == b"complete" {
                        return Ok(Some(()));
                    }
                }
            }
            Ok(None)
        })
        .unwrap();
    writer.join().unwrap();
}
#[test]
#[cfg_attr(coverage_nightly, coverage(off))]
fn registration_before_recheck_handles_preexisting_and_new_nested_paths() {
    let root = Root::new();
    let path = root.0.join("private/control.sock");
    let mut events = support::FileEvents::new(&[&path]).unwrap();
    // Creation between registration and first predicate must not be missed.
    std::fs::create_dir(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"ready").unwrap();
    events.until(|| Ok(path.exists().then_some(()))).unwrap();
    // No further event is needed for an already-satisfied predicate.
    events
        .until(|| Ok(Some(42)))
        .map(|n| assert_eq!(n, 42))
        .unwrap();
}
#[test]
#[cfg_attr(coverage_nightly, coverage(off))]
fn child_exit_is_reaped_and_output_is_drained_while_running() {
    let output = support::output(Command::new("/bin/sh").args(["-c",
        "i=0; while [ \"$i\" -lt 4096 ]; do printf '0123456789012345678901234567890123456789012345678901234567890123'; i=$((i+1)); done; printf diagnostic >&2"])).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout.len(), 4096 * 64);
    assert_eq!(output.stderr, b"diagnostic");
}
#[test]
#[cfg_attr(coverage_nightly, coverage(off))]
fn injected_outer_guard_failure_still_kills_and_reaps_owned_child() {
    let child = Command::new("/bin/sh")
        .args(["-c", "IFS= read -r release"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut owned = support::OwnedChild(child);
    let pid = owned.0.id() as libc::pid_t;
    // Already-expired outer guard: no elapsed-time assertion or startup race.
    assert_eq!(
        support::wait_child_until(&mut owned.0, std::time::Instant::now())
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut
    );
    drop(owned);
    let mut status = 0;
    // SAFETY: known child PID; waitpid only checks whether it remains unreaped.
    assert_eq!(
        unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
        -1
    );
    assert_eq!(
        io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
}

#[test]
#[cfg_attr(coverage_nightly, coverage(off))]
fn readiness_requires_complete_marker_and_reaps_early_exit_or_oversized_log() {
    for scenario in ["early-exit", "oversized", "unterminated"] {
        let root = Root::new();
        let pidfile = root.0.join("pid");
        let script = if scenario == "oversized" {
            "printf '%s' $$ >\"$1\"; i=0; while [ \"$i\" -lt 9000 ]; do printf x >&2; i=$((i+1)); done; IFS= read -r release"
        } else if scenario == "unterminated" {
            "printf '%s' $$ >\"$1\"; printf 'AirHound control socket listening' >&2"
        } else {
            "printf '%s' $$ >\"$1\"; printf 'not a readiness announcement\\n' >&2; exit 7"
        };
        let error = support::ServiceChild::spawn(
            Command::new("/bin/sh")
                .args(["-c", script, "fixture"])
                .arg(&pidfile)
                .stdin(Stdio::piped())
                .stdout(Stdio::null()),
        )
        .err()
        .expect("readiness must fail");
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(error.to_string().contains(match scenario {
            "oversized" => "diagnostic bound",
            "unterminated" => "unterminated",
            _ => "before listening",
        }));
        let pid: libc::pid_t = std::fs::read_to_string(pidfile).unwrap().parse().unwrap();
        let mut status = 0;
        // SAFETY: recorded fixture child PID; successful cleanup must have reaped it.
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(coverage_nightly, coverage(off))]
fn inotify_stat_add_race_and_reused_ignored_descriptor_rearm_without_removing_new_watch() {
    use std::os::unix::fs::MetadataExt;
    let root = Root::new();
    let path = root.0.join("database");
    let staged = root.0.join("replacement");
    std::fs::write(&path, b"old").unwrap();
    std::fs::write(&staged, b"new").unwrap();
    let mut events = support::FileEvents::new(&[&path]).unwrap();
    let old = events.watched[&path].identity;
    events.remove(&path);
    // Replace after stat but before inotify_add_watch, with no scheduler race.
    events
        .refresh_with(|candidate| {
            if candidate == path.as_path() {
                std::fs::rename(&staged, &path)?;
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(events.watched[&path].identity, old);
    let metadata = std::fs::metadata(&path).unwrap();
    let current = (metadata.dev(), metadata.ino());
    assert_ne!(current, old);
    events.refresh().unwrap();
    assert_eq!(events.watched[&path].identity, current);
    let current_descriptor = events.watched[&path].descriptor;
    // Model the worst case: a delayed IN_IGNORED has the same number as a new
    // live watch. Metadata invalidation must not remove that kernel watch.
    events.ignored_descriptor(current_descriptor);
    assert!(!events.watched.contains_key(&path));
    events.refresh().unwrap();
    assert_eq!(events.watched[&path].descriptor, current_descriptor);
    assert_eq!(events.watched[&path].identity, current);
    let mut wrote = false;
    events
        .until(|| {
            if !wrote {
                wrote = true;
                std::fs::write(&path, b"complete")?;
                Ok(None)
            } else {
                Ok((std::fs::read(&path)? == b"complete").then_some(()))
            }
        })
        .unwrap();
}
