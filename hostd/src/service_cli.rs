//! Thin CLI/service adapters; validation and discovery remain Rust backend operations.
use airhound_api::{ApiError, ErrorCode, MAX_CONFIG_BYTES};
use airhound_client::{Client, ClientError};
use airhound_hostd::service::{Backend, BackendResult};
use serde_json::Value;
use std::{
    io,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[derive(clap::Subcommand)]
pub(crate) enum ClientCommand {
    /// Show the connected service's capabilities (not RF readiness)
    Status,
    /// List durable recordings, newest first
    Recordings {
        #[arg(long)]
        before: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u16,
    },
    /// Read a bounded page of retained observations and evidence
    Records {
        #[arg(long)]
        run_id: String,
        #[arg(long, default_value_t = 0)]
        after: i64,
        #[arg(long, default_value_t = 100)]
        limit: u16,
    },
    /// Passively inventory device candidates on the backend host
    Devices {
        #[arg(long)]
        all: bool,
    },
    /// Validate JSON configuration on the backend without opening receivers
    Validate {
        #[arg(long)]
        session_config: PathBuf,
    },
}

#[derive(Default)]
struct LocalBackend {
    database: Option<PathBuf>,
}
impl LocalBackend {
    fn history(
        &self,
        query: impl FnOnce(airhound_hostd::storage::Reader) -> io::Result<Value> + Send + 'static,
    ) -> BackendResult {
        let path = self.database.clone();
        Box::pin(async move {
            let path = path.ok_or_else(|| {
                ApiError::new(
                    ErrorCode::UnsupportedMethod,
                    "recording history is not configured",
                )
            })?;
            tokio::task::spawn_blocking(move || {
                query(airhound_hostd::storage::Reader::open(&path)?)
            })
            .await
            .map_err(|_| ApiError::new(ErrorCode::Internal, "recording query task failed"))?
            .map_err(|error| {
                ApiError::new(
                    if error.kind() == io::ErrorKind::InvalidInput {
                        ErrorCode::InvalidRequest
                    } else {
                        ErrorCode::Internal
                    },
                    error.to_string(),
                )
            })
        })
    }
}
impl Backend for LocalBackend {
    fn recordings(&self, before: Option<String>, limit: u16) -> BackendResult {
        self.history(move |reader| {
            serde_json::to_value(reader.run_page(before.as_deref(), limit.into())?)
                .map_err(io::Error::other)
        })
    }
    fn recording_records(&self, run_id: String, after: i64, limit: u16) -> BackendResult {
        self.history(move |reader| {
            serde_json::to_value(reader.records(&run_id, after, limit.into())?)
                .map_err(io::Error::other)
        })
    }
    fn devices(&self, all: bool) -> BackendResult {
        Box::pin(async move {
            let inventory =
                airhound_hostd::discovery::discover(&airhound_hostd::discovery::Options {
                    all,
                    ..Default::default()
                })
                .await;
            serde_json::to_value(inventory)
                .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))
        })
    }
    fn validate(&self, config: Value) -> BackendResult {
        Box::pin(async move {
            let plan = serde_json::from_value(config)
                .map_err(|e| ApiError::new(ErrorCode::InvalidSession, e.to_string()))?;
            crate::session_runtime::preview(plan)
                .await
                .map_err(|e| ApiError::new(ErrorCode::InvalidSession, e.to_string()))
        })
    }
}

pub(crate) async fn serve(path: &Path, database: Option<PathBuf>) -> io::Result<()> {
    airhound_hostd::service::serve(path, Arc::new(LocalBackend { database }), async {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    })
    .await
}

fn read_config(path: &Path) -> io::Result<Value> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "session config must be a regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take((MAX_CONFIG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "session config exceeds 64 KiB",
        ));
    }
    // Syntax only: no client-side SessionConfig, resolution, reservation or policy logic.
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

pub(crate) async fn client(path: &Path, command: ClientCommand) -> io::Result<()> {
    let mut client = Client::connect(path, Duration::from_secs(20))
        .await
        .map_err(io::Error::other)?;
    let result = match command {
        ClientCommand::Status => client.status().await,
        ClientCommand::Recordings { before, limit } => client.recordings(before, limit).await,
        ClientCommand::Records {
            run_id,
            after,
            limit,
        } => client.recording_records(run_id, after, limit).await,
        ClientCommand::Devices { all } => client.devices(all).await,
        ClientCommand::Validate { session_config } => {
            client.validate(read_config(&session_config)?).await
        }
    };
    match result {
        Ok(value) => crate::observation::write_json(&mut io::stdout(), &value),
        Err(ClientError::Remote(error)) => {
            crate::observation::write_json(&mut io::stdout(), &serde_json::json!({"error":error}))?;
            Err(io::Error::other(error))
        }
        Err(error) => Err(io::Error::other(error)),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    #[tokio::test]
    async fn backend_preview_reuses_canonical_validation() {
        let config = serde_json::json!({"version":1,"receivers":[{"id":"one","source":{"kind":"native_ble"}}]});
        let expected =
            crate::session_runtime::preview(serde_json::from_value(config.clone()).unwrap())
                .await
                .unwrap();
        assert_eq!(
            LocalBackend::default().validate(config).await.unwrap(),
            expected
        );
        assert_eq!(
            LocalBackend::default()
                .validate(serde_json::json!({"version":9,"receivers":[]}))
                .await
                .unwrap_err()
                .code,
            ErrorCode::InvalidSession
        );
    }
}
