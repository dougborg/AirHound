//! Cooperative serial ownership shared with scripts/sniffle-source.py.
//!
//! Acquire before opening a serial transport or spawning its owner. Hold the
//! returned guard until the transport/child is closed, including pause/recovery.
//! Sniffle may borrow its supervisor's inherited descriptor; direct helpers acquire their own.
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

fn canonical_identity(port: &Path) -> io::Result<String> {
    let canonical = fs::canonicalize(port)?;
    if !fs::metadata(&canonical)?.file_type().is_char_device() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "serial endpoint must be a character device",
        ));
    }
    let text = canonical.to_str().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "serial endpoint must be UTF-8")
    })?;
    Ok(normalize(text))
}
fn normalize(canonical: &str) -> String {
    canonical.strip_prefix("/dev/cu.").map_or_else(
        || canonical.to_owned(),
        |suffix| format!("/dev/tty.{suffix}"),
    )
}
fn key(identity: &str) -> String {
    Sha256::digest(identity.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
        + ".lock"
}
/// Same fixed namespace, identity normalization and key algorithm as Python.
/// This coordinates updated AirHound processes for one UID, not unrelated tools.
#[derive(Debug)]
pub struct Lease {
    file: File,
    identity: String,
}
impl Lease {
    /// Inherit only into the owned helper, clearing CLOEXEC in its pre-exec hook.
    pub fn raw_fd(&self) -> std::os::fd::RawFd {
        self.file.as_raw_fd()
    }
    /// A reappearing configured path must still name the originally leased endpoint.
    pub fn matches(&self, port: impl AsRef<Path>) -> io::Result<bool> {
        Ok(canonical_identity(port.as_ref())? == self.identity)
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        // Explicit unlock also releases a briefly inherited fork descriptor before
        // exec closes it; close alone can retain the lock until that child execs.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
pub fn acquire(port: impl AsRef<Path>) -> io::Result<Lease> {
    let identity = canonical_identity(port.as_ref())?;
    let uid = unsafe { libc::geteuid() };
    let directory = PathBuf::from("/tmp").join(format!("airhound-serial-{uid}"));
    acquire_in(&directory, &identity, uid)
}
/// Cooperative selected-device ownership for managed ADB subscriptions. This
/// shares the proven private lock implementation, not a physical serial port.
pub fn acquire_adb(serial: &str) -> io::Result<Lease> {
    let uid = unsafe { libc::geteuid() };
    let directory = PathBuf::from("/tmp").join(format!("airhound-serial-{uid}"));
    acquire_in(&directory, &format!("adb-usb:{serial}:tcp:8082"), uid)
}
fn acquire_in(directory: &Path, identity: &str, uid: u32) -> io::Result<Lease> {
    match fs::DirBuilder::new().mode(0o700).create(directory) {
        Ok(()) => (),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e),
    }
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != uid
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "serial lease directory must be owned and private (0700)",
        ));
    }
    // Never unlink locks: another process may still hold the original inode.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(directory.join(key(identity)))?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o777 != 0o600 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "serial lease must be an owned private regular file",
        ));
    }
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "serial endpoint is already owned by another AirHound source",
        ));
    }
    Ok(Lease {
        file,
        identity: identity.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::owned_child::OwnedChild;
    use std::process::Stdio;
    use tokio::{
        io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
        process::{ChildStdout, Command},
    };
    static NEXT_FIXTURE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    async fn read_line(output: &mut BufReader<ChildStdout>) -> io::Result<String> {
        let mut line = Vec::new();
        output.take(4097).read_until(b'\n', &mut line).await?;
        if line.len() > 4096 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "test helper line exceeds limit",
            ));
        }
        if line.last() != Some(&b'\n') {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "test helper closed stdout",
            ));
        }
        String::from_utf8(line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
    #[tokio::test]
    async fn idle_test_child_is_cancelled_and_reaped_on_request() {
        crate::test_support::manual_time(async {
            let process = Command::new("python3")
                .args([
                    "-c",
                    "import signal;print('ready',flush=True);signal.pause()",
                ])
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let pid = process.id().unwrap();
            let mut child = OwnedChild::new(process, None).unwrap();
            let mut output = BufReader::new(child.stdout.take().unwrap());
            assert_eq!(read_line(&mut output).await.unwrap(), "ready\n");
            assert!(!child.finish().await.unwrap().success());
            assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        })
        .await;
    }
    #[test]
    fn alias_keys_match_python_contract() {
        assert_eq!(
            normalize("/dev/cu.usbmodem1"),
            normalize("/dev/tty.usbmodem1")
        );
        assert_eq!(
            key("/dev/tty.fixture"),
            "b506c2e5b84d5469586db00ab48e09f5cd55eae38bc1c7ff43c0bc5867fbf504.lock"
        );
    }
    #[test]
    fn unsafe_lock_nodes_are_rejected_without_device_open() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "airhound-lease-fixture-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let uid = unsafe { libc::geteuid() };
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let path = root.join(key("fixture"));
        std::os::unix::fs::symlink("missing", &path).unwrap();
        assert!(acquire_in(&root, "fixture", uid).is_err());
        fs::remove_file(&path).unwrap();
        let name = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(acquire_in(&root, "fixture", uid).is_err());
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(acquire_in(&root, "fixture", uid).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let held = acquire_in(&root, "fixture", uid).unwrap();
        assert!(acquire_in(&root, "fixture", uid).is_err());
        let inherited = held.file.try_clone().unwrap();
        drop(held);
        // Model an fd inherited briefly by a concurrent fork before exec.
        // Releasing the owner must release the lease even while that fd exists.
        assert!(acquire_in(&root, "fixture", uid).is_ok());
        drop(inherited);
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(acquire_in(&root, "fixture", uid).is_err());
        assert!(acquire(&path).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn python_and_rust_owners_exclude_each_other_before_uart_access() {
        crate::test_support::manual_time(async {
        let script = r#"
import importlib.util,os,pty,sys
s=importlib.util.spec_from_file_location('helper',sys.argv[1]);h=importlib.util.module_from_spec(s);s.loader.exec_module(h)
m,slave=pty.openpty();port=os.ttyname(slave)
if sys.argv[2]=='python_first':
 with h.port_lease(port):
  print(port,flush=True);sys.stdin.readline()
 print('released',flush=True);sys.stdin.readline()
else:
 print(port,flush=True);sys.stdin.readline()
 try:
  with h.port_lease(port):
   fd=os.open(port,os.O_RDWR|os.O_NOCTTY);os.write(fd,b'BAD');os.close(fd)
  print('incorrectly_acquired',flush=True)
 except RuntimeError:
  os.set_blocking(m,False)
  try:os.read(m,1);result='uart_bytes'
  except BlockingIOError:result='denied_without_bytes'
  print(result,flush=True)
 sys.stdin.readline()
os.close(m);os.close(slave)
"#;
        for mode in ["python_first", "rust_first"] {
            let helper = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/sniffle-source.py");
            let mut process = Command::new("python3")
                    .args(["-B", "-c", script])
                    .arg(helper)
                    .arg(mode)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .kill_on_drop(true)
                    .spawn()
                    .unwrap();
            let mut input = process.stdin.take().unwrap();
            let mut child = OwnedChild::new(process, None).unwrap();
            let mut output = BufReader::new(child.stdout.take().unwrap());
            let line = read_line(&mut output).await.unwrap();
            let port = line.trim();
            if mode == "python_first" {
                assert_eq!(acquire(port).unwrap_err().kind(), io::ErrorKind::WouldBlock);
                input.write_all(b"release\n").await.unwrap();
                let released = read_line(&mut output).await.unwrap();
                assert_eq!(released.trim(), "released");
                let _lease = acquire(port).unwrap();
                input.write_all(b"finish\n").await.unwrap();
            } else {
                let _lease = acquire(port).unwrap();
                input.write_all(b"attempt\n").await.unwrap();
                let response = read_line(&mut output).await.unwrap();
                assert_eq!(response.trim(), "denied_without_bytes");
                input.write_all(b"finish\n").await.unwrap();
            }
            assert!(child.wait().await.unwrap().success());
        }
        }).await;
    }
}
