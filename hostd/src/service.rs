//! Framed local control service. Backend operations own capture and identification;
//! this transport does not construct scanners or open device endpoints.
use airhound_api::{
    ApiError, Command, ErrorCode, Hello, Request, Response, MAX_CONFIG_BYTES, MAX_FRAME_BYTES,
    METHODS, VERSION,
};
use airhound_client::{read_frame, write_frame};
use serde_json::{json, Value};
use std::{
    future::Future,
    io,
    os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::{UnixListener, UnixStream},
    task::JoinSet,
};

const CLIENT_LIMIT: usize = 16;
const FRAME_DEADLINE: Duration = Duration::from_secs(30);
const OPERATION_DEADLINE: Duration = Duration::from_secs(15);
const WRITE_DEADLINE: Duration = Duration::from_secs(5);

pub type BackendResult = Pin<Box<dyn Future<Output = Result<Value, ApiError>> + Send>>;
pub trait Backend: Send + Sync + 'static {
    fn profiles(&self, _command: Command) -> BackendResult {
        Box::pin(async {
            Err(ApiError::new(
                ErrorCode::UnsupportedMethod,
                "saved profiles are not configured",
            ))
        })
    }
    fn export_recordings(&self, _destination: String) -> BackendResult {
        Box::pin(async {
            Err(ApiError::new(
                ErrorCode::UnsupportedMethod,
                "recording history is not configured",
            ))
        })
    }
    fn capture_enabled(&self) -> bool {
        false
    }
    fn capture(&self, _action: airhound_api::CaptureAction) -> BackendResult {
        Box::pin(async {
            Err(ApiError::new(
                ErrorCode::UnsupportedMethod,
                "capture control is not enabled",
            ))
        })
    }
    fn identify_device(&self, _config: Value) -> BackendResult {
        Box::pin(async {
            Err(ApiError::new(
                ErrorCode::UnsupportedMethod,
                "device identification requires a capture-capable service",
            ))
        })
    }
    fn devices(&self, all: bool) -> BackendResult;
    fn validate(&self, config: Value) -> BackendResult;
    fn recordings(&self, _before: Option<String>, _limit: u16) -> BackendResult {
        Box::pin(async {
            Err(ApiError::new(
                ErrorCode::UnsupportedMethod,
                "recording history is not configured",
            ))
        })
    }
    fn recording_records(&self, _run_id: String, _after: i64, _limit: u16) -> BackendResult {
        Box::pin(async {
            Err(ApiError::new(
                ErrorCode::UnsupportedMethod,
                "recording history is not configured",
            ))
        })
    }
}

struct SocketFile {
    path: PathBuf,
    dev: u64,
    ino: u64,
}
impl Drop for SocketFile {
    fn drop(&mut self) {
        // Never remove a replacement file/socket created at our old pathname.
        if std::fs::symlink_metadata(&self.path)
            .is_ok_and(|m| m.file_type().is_socket() && m.dev() == self.dev && m.ino() == self.ino)
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// The immediate parent must be owned by this user, nonsymlink and mode 0700.
/// Existing socket paths are never unlinked, including stale endpoints.
fn bind_private(path: &Path) -> io::Result<(UnixListener, SocketFile)> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket path must be absolute",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket requires a private parent directory",
        )
    })?;
    match std::fs::DirBuilder::new().mode(0o700).create(parent) {
        Ok(()) => (),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e),
    }
    let metadata = std::fs::symlink_metadata(parent)?;
    // SAFETY: geteuid has no preconditions and does not mutate process state.
    let uid = unsafe { libc::geteuid() };
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != uid
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "socket parent must be an owned, nonsymlink directory with mode 0700",
        ));
    }
    let listener = UnixListener::bind(path)?;
    let metadata = std::fs::symlink_metadata(path)?;
    let owned = SocketFile {
        path: path.into(),
        dev: metadata.dev(),
        ino: metadata.ino(),
    };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok((listener, owned))
}

/// Bounded local service with concurrent clients. Shutdown aborts only passive
/// requests and awaits their futures; there is no capture lifetime in this slice.
pub async fn serve(
    path: &Path,
    backend: Arc<dyn Backend>,
    shutdown: impl Future<Output = ()>,
) -> io::Result<()> {
    let (listener, _owned_socket) = bind_private(path)?;
    let hello = Arc::new(Hello {
        version: VERSION,
        // A per-process instance marker, not an authentication token.
        instance_id: format!(
            "{:x}-{:x}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ),
        build_version: crate::build_info::current()["display_version"]
            .as_str()
            .unwrap_or("source revision unknown")
            .into(),
        max_frame_bytes: MAX_FRAME_BYTES,
        methods: METHODS.iter().map(|s| (*s).into()).collect(),
        mode: if backend.capture_enabled() {
            "capture_control"
        } else {
            "passive_control"
        }
        .into(),
    });
    tokio::pin!(shutdown);
    let mut clients = JoinSet::new();
    let result = loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => break Ok(()),
            Some(_) = clients.join_next(), if !clients.is_empty() => {},
            accepted = listener.accept() => {
                let (stream, _) = match accepted { Ok(value) => value, Err(error) => break Err(error) };
                // All initial clients must have the service's effective UID. There is no remote listener.
                let same_user = stream.peer_cred().is_ok_and(|peer| peer.uid() == unsafe { libc::geteuid() });
                if !same_user || clients.len() >= CLIENT_LIMIT { drop(stream); continue; }
                let backend = backend.clone(); let hello = hello.clone();
                clients.spawn(async move { connection(stream, backend, hello).await });
            }
        }
    };
    clients.abort_all();
    while clients.join_next().await.is_some() {}
    result
}

async fn connection(mut stream: UnixStream, backend: Arc<dyn Backend>, hello: Arc<Hello>) {
    let mut handshaken = false;
    let mut previous_id = 0;
    loop {
        let bytes = match tokio::time::timeout(FRAME_DEADLINE, read_frame(&mut stream)).await {
            Ok(Ok(bytes)) => bytes,
            _ => return,
        };
        let raw: Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => {
                let _ = reply(
                    &mut stream,
                    Response::failure(
                        0,
                        ApiError::new(ErrorCode::InvalidRequest, "frame is not valid JSON"),
                    ),
                )
                .await;
                return;
            }
        };
        let id = raw.get("request_id").and_then(Value::as_u64).unwrap_or(0);
        if id == 0 || id <= previous_id {
            let _ = reply(
                &mut stream,
                Response::failure(
                    id,
                    ApiError::new(
                        ErrorCode::InvalidRequest,
                        "request_id must be nonzero and increase within a connection",
                    ),
                ),
            )
            .await;
            return;
        }
        previous_id = id;
        if raw.get("version").and_then(Value::as_u64) != Some(VERSION.into()) {
            let _ = reply(
                &mut stream,
                Response::failure(
                    id,
                    ApiError::new(
                        ErrorCode::IncompatibleVersion,
                        "control protocol version 1 is required",
                    ),
                ),
            )
            .await;
            return;
        }
        let request: Request = match serde_json::from_value(raw.clone()) {
            Ok(value) => value,
            Err(error) => {
                let unknown = raw
                    .pointer("/command/method")
                    .and_then(Value::as_str)
                    .is_some_and(|m| !METHODS.contains(&m));
                let code = if unknown {
                    ErrorCode::UnsupportedMethod
                } else {
                    ErrorCode::InvalidRequest
                };
                if reply(
                    &mut stream,
                    Response::failure(id, ApiError::new(code, error.to_string())),
                )
                .await
                .is_err()
                {
                    return;
                }
                continue;
            }
        };
        let result = match request.command {
            Command::Hello {} => {
                handshaken = true;
                Ok(serde_json::to_value(&*hello).expect("serializable hello"))
            }
            _ if !handshaken => Err(ApiError::new(
                ErrorCode::HandshakeRequired,
                "send hello before other commands",
            )),
            Command::Status {} => Ok(
                json!({"instance_id": hello.instance_id, "mode":if backend.capture_enabled() { "capture_control" } else { "passive_control" }, "capture_control":backend.capture_enabled(), "methods": METHODS, "build":crate::build_info::current(), "location_providers":["gpsd"], "managed_helpers":crate::managed_helpers::default_status()}),
            ),
            Command::Capture { action } => bounded(backend.capture(action)).await,
            command @ (Command::ProfilesList {}
            | Command::ProfilesLoad { .. }
            | Command::ProfilesDelete { .. }
            | Command::ProfilesSave { .. }) => {
                if matches!(&command, Command::ProfilesSave { config, .. } if serde_json::to_vec(config).map_or(true, |bytes| bytes.len() > MAX_CONFIG_BYTES))
                {
                    Err(ApiError::new(
                        ErrorCode::InvalidRequest,
                        "profile config exceeds 64 KiB",
                    ))
                } else {
                    bounded(backend.profiles(command)).await
                }
            }
            Command::RecordingExport { destination } => {
                bounded(backend.export_recordings(destination)).await
            }
            Command::DevicesList { all } => bounded(backend.devices(all)).await,
            Command::DevicesIdentify { config } => {
                if serde_json::to_vec(&config).map_or(true, |bytes| bytes.len() > MAX_CONFIG_BYTES)
                {
                    Err(ApiError::new(
                        ErrorCode::InvalidRequest,
                        "identification config exceeds 64 KiB",
                    ))
                } else {
                    bounded(backend.identify_device(config)).await
                }
            }
            Command::RecordingsList { before, limit } => {
                bounded(backend.recordings(before, limit)).await
            }
            Command::RecordingRecords {
                run_id,
                after,
                limit,
            } => bounded(backend.recording_records(run_id, after, limit)).await,
            Command::SessionValidate { config } => {
                if serde_json::to_vec(&config).map_or(true, |bytes| bytes.len() > MAX_CONFIG_BYTES)
                {
                    Err(ApiError::new(
                        ErrorCode::InvalidSession,
                        "session config exceeds 64 KiB",
                    ))
                } else {
                    bounded(backend.validate(config)).await
                }
            }
        };
        let response = match result {
            Ok(value) => Response::success(id, value),
            Err(error) => Response::failure(id, error),
        };
        if reply(&mut stream, response).await.is_err() {
            return;
        }
    }
}
async fn bounded(future: BackendResult) -> Result<Value, ApiError> {
    tokio::time::timeout(OPERATION_DEADLINE, future)
        .await
        .unwrap_or_else(|_| {
            Err(ApiError::new(
                ErrorCode::Timeout,
                "backend operation timed out",
            ))
        })
}
async fn reply(stream: &mut UnixStream, response: Response) -> io::Result<()> {
    tokio::time::timeout(WRITE_DEADLINE, write_frame(stream, &response))
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "client did not consume control response",
            )
        })?
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use airhound_client::Client;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ah-api-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .unwrap();
            Self(path)
        }
        fn socket(&self) -> PathBuf {
            self.0.join("s")
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    struct Fake;
    impl Backend for Fake {
        fn profiles(&self, command: Command) -> BackendResult {
            Box::pin(async move { Ok(serde_json::to_value(command).unwrap()) })
        }
        fn devices(&self, all: bool) -> BackendResult {
            Box::pin(async move {
                Ok(json!({"schema_version":1,"mode":"passive","devices":[],"all":all}))
            })
        }
        fn validate(&self, config: Value) -> BackendResult {
            Box::pin(async move {
                if config.get("version") == Some(&json!(1)) {
                    Ok(json!({"valid":true,"readiness":"unverified"}))
                } else {
                    Err(ApiError::new(ErrorCode::InvalidSession, "invalid version"))
                }
            })
        }
    }
    async fn connect(path: &Path) -> Client {
        for _ in 0..100 {
            if let Ok(client) = Client::connect(path, Duration::from_secs(1)).await {
                return client;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("service did not become available")
    }
    #[tokio::test]
    async fn two_clients_share_service_and_detach_does_not_stop_it() {
        let dir = Directory::new();
        let path = dir.socket();
        let server_path = path.clone();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            serve(&server_path, Arc::new(Fake), async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
        });
        let mut a = connect(&path).await;
        let mut b = connect(&path).await;
        assert_eq!(a.hello.instance_id, b.hello.instance_id);
        assert_eq!(a.devices(true).await.unwrap()["all"], true);
        assert_eq!(a.list_profiles().await.unwrap()["method"], "profiles.list");
        let saved = a
            .save_profile(
                "fixture".into(),
                "Fixture".into(),
                json!({"version":1}),
                false,
            )
            .await
            .unwrap();
        assert_eq!(saved["method"], "profiles.save");
        assert_eq!(saved["params"]["replace"], false);
        assert_eq!(
            a.load_profile("fixture".into()).await.unwrap()["params"]["id"],
            "fixture"
        );
        assert_eq!(
            a.delete_profile("fixture".into()).await.unwrap()["method"],
            "profiles.delete"
        );
        assert!(matches!(
            a.save_profile(
                "large".into(),
                "Large".into(),
                json!({"padding":"x".repeat(MAX_CONFIG_BYTES)}),
                false
            )
            .await,
            Err(airhound_client::ClientError::Remote(ApiError {
                code: ErrorCode::InvalidRequest,
                ..
            }))
        ));
        assert!(matches!(
            a.validate(json!({"version":1,"padding":"x".repeat(MAX_CONFIG_BYTES)}))
                .await,
            Err(airhound_client::ClientError::Remote(ApiError {
                code: ErrorCode::InvalidSession,
                ..
            }))
        ));
        assert_eq!(
            b.validate(json!({"version":1})).await.unwrap()["readiness"],
            "unverified"
        );
        assert!(matches!(
            b.validate(json!({})).await,
            Err(airhound_client::ClientError::Remote(ApiError {
                code: ErrorCode::InvalidSession,
                ..
            }))
        ));
        drop(a);
        assert_eq!(b.status().await.unwrap()["capture_control"], false);
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        stop.send(()).unwrap();
        task.await.unwrap();
        assert!(!path.exists());
    }
    #[tokio::test]
    async fn endpoint_security_never_unlinks_existing_paths() {
        let dir = Directory::new();
        let path = dir.socket();
        std::fs::write(&path, b"keep").unwrap();
        assert!(bind_private(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"keep");
        std::fs::remove_file(&path).unwrap();
        std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(bind_private(&path).is_err());
        std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (listener, guard) = bind_private(&path).unwrap();
        assert!(bind_private(&path).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"replacement").unwrap();
        drop(listener);
        drop(guard);
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
    }
    #[tokio::test]
    async fn pending_client_does_not_block_another_and_shutdown_cancels_it() {
        struct Waiting(Arc<tokio::sync::Notify>);
        impl Backend for Waiting {
            fn devices(&self, _: bool) -> BackendResult {
                Box::pin(async { Ok(json!({})) })
            }
            fn validate(&self, _: Value) -> BackendResult {
                let entered = self.0.clone();
                Box::pin(async move {
                    entered.notify_one();
                    std::future::pending().await
                })
            }
        }
        let dir = Directory::new();
        let path = dir.socket();
        let server_path = path.clone();
        let entered = Arc::new(tokio::sync::Notify::new());
        let backend = Arc::new(Waiting(entered.clone()));
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            serve(&server_path, backend, async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
        });
        let mut a = connect(&path).await;
        let mut b = connect(&path).await;
        let pending = tokio::spawn(async move { a.validate(json!({})).await });
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .unwrap();
        assert_eq!(b.status().await.unwrap()["mode"], "passive_control");
        stop.send(()).unwrap();
        server.await.unwrap();
        assert!(pending.await.unwrap().is_err());
        assert!(!path.exists());
    }
    #[tokio::test]
    async fn protocol_rejects_capture_unknown_version_and_reused_ids() {
        let dir = Directory::new();
        let path = dir.socket();
        let server_path = path.clone();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            serve(&server_path, Arc::new(Fake), async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
        });
        drop(connect(&path).await);
        let mut stream = UnixStream::connect(&path).await.unwrap();
        for (id, command, code) in [
            (
                1,
                json!({"method":"status","params":{}}),
                ErrorCode::HandshakeRequired,
            ),
            (
                2,
                json!({"method":"session.start","params":{}}),
                ErrorCode::UnsupportedMethod,
            ),
        ] {
            write_frame(
                &mut stream,
                &json!({"version":1,"request_id":id,"command":command}),
            )
            .await
            .unwrap();
            let response: Response =
                serde_json::from_slice(&read_frame(&mut stream).await.unwrap()).unwrap();
            assert_eq!(response.error.unwrap().code, code);
        }
        write_frame(
            &mut stream,
            &json!({"version":1,"request_id":2,"command":{"method":"hello","params":{}}}),
        )
        .await
        .unwrap();
        let response: Response =
            serde_json::from_slice(&read_frame(&mut stream).await.unwrap()).unwrap();
        assert_eq!(response.error.unwrap().code, ErrorCode::InvalidRequest);
        let mut stream = UnixStream::connect(&path).await.unwrap();
        write_frame(
            &mut stream,
            &json!({"version":9,"request_id":1,"command":{"method":"hello","params":{}}}),
        )
        .await
        .unwrap();
        let response: Response =
            serde_json::from_slice(&read_frame(&mut stream).await.unwrap()).unwrap();
        assert_eq!(response.error.unwrap().code, ErrorCode::IncompatibleVersion);
        stop.send(()).unwrap();
        task.await.unwrap();
    }
}
