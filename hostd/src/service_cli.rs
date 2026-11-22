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

struct LocalBackend;
impl Backend for LocalBackend {
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

pub(crate) async fn serve(path: &Path) -> io::Result<()> {
    airhound_hostd::service::serve(path, Arc::new(LocalBackend), async {
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
mod tests {
    use super::*;
    #[tokio::test]
    async fn backend_preview_reuses_canonical_validation() {
        let config = serde_json::json!({"version":1,"receivers":[{"id":"one","source":{"kind":"native_ble"}}]});
        let expected =
            crate::session_runtime::preview(serde_json::from_value(config.clone()).unwrap())
                .await
                .unwrap();
        assert_eq!(LocalBackend.validate(config).await.unwrap(), expected);
        assert_eq!(
            LocalBackend
                .validate(serde_json::json!({"version":9,"receivers":[]}))
                .await
                .unwrap_err()
                .code,
            ErrorCode::InvalidSession
        );
    }
}
