//! Presentation requests use the shared client; no transport or planner copy.
use airhound_client::Client;
use serde_json::Value;
use std::{io, path::Path, time::Duration};
use tokio::io::AsyncReadExt;

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
pub const OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
const CONFIG_LIMIT: u64 = airhound_api::MAX_CONFIG_BYTES as u64;

pub struct Snapshot {
    pub instance: String,
    pub build: String,
    pub mode: String,
    pub status: Value,
    pub inventory: Value,
}

pub async fn refresh(socket: &Path) -> Result<Snapshot, String> {
    tokio::time::timeout(OPERATION_TIMEOUT, async {
        let mut client = Client::connect(socket, REQUEST_TIMEOUT)
            .await
            .map_err(|e| e.to_string())?;
        let status = client.status().await.map_err(|e| e.to_string())?;
        let inventory = client.devices(false).await.map_err(|e| e.to_string())?;
        if !inventory["devices"].is_array() {
            return Err("Backend inventory does not contain a device list".into());
        }
        Ok(Snapshot {
            instance: client.hello.instance_id,
            build: client.hello.build_version,
            mode: client.hello.mode,
            status,
            inventory,
        })
    })
    .await
    .map_err(|_| "Refresh timed out; press r to retry".to_owned())?
}

async fn read_config(path: &Path) -> io::Result<Value> {
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    let file = options.open(path).await?;
    if !file.metadata().await?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Session input must be a regular JSON file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(CONFIG_LIMIT + 1).read_to_end(&mut bytes).await?;
    if bytes.len() as u64 > CONFIG_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Session input exceeds 64 KiB",
        ));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

pub async fn validate(socket: &Path, config: &Path) -> Result<Value, String> {
    tokio::time::timeout(OPERATION_TIMEOUT, async {
        let config = read_config(config).await.map_err(|e| e.to_string())?;
        let mut client = Client::connect(socket, REQUEST_TIMEOUT)
            .await
            .map_err(|e| e.to_string())?;
        client.validate(config).await.map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "Validation timed out; press v to retry".to_owned())?
}

#[cfg(test)]
#[path = "../../../hostd/crates/airhound-client/src/test_clock.rs"]
mod test_clock;

#[cfg(test)]
mod tests {
    use super::*;
    use airhound_api::{Command, Hello, Request, Response, MAX_FRAME_BYTES, METHODS, VERSION};
    use airhound_client::{read_frame, write_frame};
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    use tokio::net::UnixListener;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ah-tui-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn socket(&self) -> PathBuf {
            self.0.join("service.sock")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn hello() -> Value {
        serde_json::to_value(Hello {
            version: VERSION,
            instance_id: "fake-service".into(),
            build_version: "test".into(),
            max_frame_bytes: MAX_FRAME_BYTES,
            methods: METHODS.iter().map(|s| (*s).into()).collect(),
            mode: "passive_control".into(),
        })
        .unwrap()
    }
    #[tokio::test(start_paused = true)]
    async fn refresh_uses_shared_client_and_only_passive_methods() {
        let time = super::test_clock::ManualTime::hold().await;
        let fixture = Fixture::new();
        let listener = UnixListener::bind(fixture.socket()).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            for method in ["hello", "status", "devices"] {
                let request: Request =
                    serde_json::from_slice(&read_frame(&mut stream).await.unwrap()).unwrap();
                let response = match (method, request.command) {
                    ("hello", Command::Hello {}) => hello(),
                    ("status", Command::Status {}) => serde_json::json!({"capture_control":false}),
                    ("devices", Command::DevicesList { all: false }) => {
                        serde_json::json!({"devices":[{"id":"fixture","name":"Synthetic receiver","candidates":[{"state":"unverified"}]}],"diagnostics":[]})
                    }
                    _ => panic!("unexpected non-passive request"),
                };
                write_frame(
                    &mut stream,
                    &Response::success(request.request_id, response),
                )
                .await
                .unwrap();
            }
        });
        let snapshot = refresh(&fixture.socket()).await.unwrap();
        assert_eq!(snapshot.instance, "fake-service");
        assert_eq!(snapshot.inventory["devices"][0]["id"], "fixture");
        assert_eq!(snapshot.status["capture_control"], false);
        server.await.unwrap();
        time.finish().await;
    }
    #[tokio::test(start_paused = true)]
    async fn validation_forwards_json_without_a_local_planner() {
        let time = super::test_clock::ManualTime::hold().await;
        let fixture = Fixture::new();
        let listener = UnixListener::bind(fixture.socket()).unwrap();
        let path = fixture.0.join("session.json");
        let config = serde_json::json!({"version":1,"receivers":[]});
        std::fs::write(&path, config.to_string()).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request: Request =
                serde_json::from_slice(&read_frame(&mut stream).await.unwrap()).unwrap();
            assert!(matches!(request.command, Command::Hello {}));
            write_frame(&mut stream, &Response::success(request.request_id, hello()))
                .await
                .unwrap();
            let request: Request =
                serde_json::from_slice(&read_frame(&mut stream).await.unwrap()).unwrap();
            match request.command {
                Command::SessionValidate { config: received } => assert_eq!(received, config),
                _ => panic!("not validation"),
            }
            write_frame(
                &mut stream,
                &Response::failure(
                    request.request_id,
                    airhound_api::ApiError::new(
                        airhound_api::ErrorCode::InvalidSession,
                        "receiver list empty",
                    ),
                ),
            )
            .await
            .unwrap();
        });
        assert!(validate(&fixture.socket(), &path)
            .await
            .unwrap_err()
            .contains("receiver list empty"));
        server.await.unwrap();
        time.finish().await;
    }
    #[tokio::test(start_paused = true)]
    async fn missing_service_and_bounded_regular_config_fail_cleanly() {
        let time = super::test_clock::ManualTime::hold().await;
        let fixture = Fixture::new();
        assert!(refresh(&fixture.socket()).await.is_err());
        let path = fixture.0.join("oversized.json");
        std::fs::write(&path, vec![b' '; CONFIG_LIMIT as usize + 1]).unwrap();
        assert!(read_config(&path)
            .await
            .unwrap_err()
            .to_string()
            .contains("64 KiB"));
        assert!(read_config(&fixture.0).await.is_err());
        use std::os::unix::ffi::OsStrExt;
        let fifo = fixture.0.join("input.fifo");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        // A nonblocking open plus regular-file validation must reject the FIFO.
        // The outer test runner guards hangs; no one-second speed assertion.
        assert_eq!(
            read_config(&fifo).await.unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        time.finish().await;
    }
}
