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
    /// Save or reuse backend-owned session setup without opening receivers
    Profiles {
        #[command(subcommand)]
        action: ProfileCommand,
    },
    /// Export a consistent standalone SQLite snapshot on the backend host
    Export {
        #[arg(long)]
        output: String,
    },
    /// Start a durable session on a service launched with --capture
    Start {
        /// Stable id for retries of this start request; use a new id for a new run
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        session_config: PathBuf,
    },
    Pause {
        #[arg(long)]
        run_id: String,
    },
    Resume {
        #[arg(long)]
        run_id: String,
    },
    Finish {
        #[arg(long)]
        run_id: String,
    },
    CaptureStatus,
    /// Show the connected service's capabilities (not RF readiness)
    Status,
    /// Wait for authenticated Hello and Status without starting capture
    WaitReady {
        #[arg(long, default_value_t = 5000, value_parser = clap::value_parser!(u64).range(1..=60_000))]
        timeout_ms: u64,
    },
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
    /// Explicitly query Sniffle firmware; may open the selected protocol endpoint
    Identify {
        #[arg(long)]
        config: PathBuf,
    },
    /// Validate JSON configuration on the backend without opening receivers
    Validate {
        #[arg(long)]
        session_config: PathBuf,
    },
}

#[derive(clap::Subcommand)]
pub(crate) enum ProfileCommand {
    List,
    Save {
        #[arg(long)]
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        session_config: PathBuf,
        #[arg(long)]
        replace: bool,
    },
    Load {
        #[arg(long)]
        id: String,
    },
    Delete {
        #[arg(long)]
        id: String,
    },
}

#[derive(Default)]
struct LocalBackend {
    database: Option<PathBuf>,
    capture: Option<crate::capture_service::Handle>,
    profiles: Option<PathBuf>,
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
    fn profiles(&self, command: airhound_api::Command) -> BackendResult {
        let directory = self.profiles.clone();
        Box::pin(async move {
            use airhound_api::Command;
            use airhound_hostd::{discovery, profiles};
            let directory = directory.ok_or_else(|| {
                ApiError::new(
                    ErrorCode::UnsupportedMethod,
                    "saved profiles are not configured",
                )
            })?;
            let error = |e: io::Error| {
                let mut result = ApiError::new(ErrorCode::InvalidRequest, e.to_string());
                result.retryable = e.kind() == io::ErrorKind::WouldBlock;
                result
            };
            match command {
                Command::ProfilesSave {
                    id,
                    name,
                    config,
                    replace,
                } => {
                    // Canonical preflight inspects file metadata without invoking
                    // helper programs or opening serial/radio transports.
                    let plan = airhound_hostd::session::SessionConfig::from_json(config.clone())
                        .map_err(error)?;
                    let validation = crate::session_runtime::preview(plan).await.map_err(error)?;
                    let resources = profiles::check_resources(&config);
                    if resources["valid"] != true {
                        return Err(ApiError::new(
                            ErrorCode::InvalidSession,
                            format!("profile resources need attention: {resources}"),
                        ));
                    }
                    let inventory = discovery::discover(&Default::default()).await;
                    let profile = tokio::task::spawn_blocking(move || {
                        profiles::Store::open(&directory)?
                            .save(id, name, config, &inventory, replace)
                    })
                    .await
                    .map_err(|_| ApiError::new(ErrorCode::Internal, "profile task failed"))?
                    .map_err(error)?;
                    Ok(
                        serde_json::json!({"schema_version":1,"profile":profile,"validation":validation,"resources":resources,"reception":"not_tested"}),
                    )
                }
                Command::ProfilesLoad { id } => {
                    let (profile, origin_check) = tokio::task::spawn_blocking(move || {
                        let store = profiles::Store::open(&directory)?;
                        let profile = store.load(&id)?;
                        let origin_check = store.origin_check(&profile);
                        Ok::<_, io::Error>((profile, origin_check))
                    })
                    .await
                    .map_err(|_| ApiError::new(ErrorCode::Internal, "profile task failed"))?
                    .map_err(error)?;
                    let plan =
                        airhound_hostd::session::SessionConfig::from_json(profile.config.clone())
                            .map_err(error)?;
                    // Missing local resources are a useful load result, not a reason
                    // to hide the stored plan that the user needs to repair.
                    let validation = match crate::session_runtime::preview(plan).await {
                        Ok(result) => result,
                        Err(error) => {
                            serde_json::json!({"valid":false,"readiness":"unverified","error":error.to_string()})
                        }
                    };
                    let inventory = discovery::discover(&Default::default()).await;
                    let bindings = profiles::check_bindings(&profile, &inventory);
                    let resources = profiles::check_resources(&profile.config);
                    Ok(
                        serde_json::json!({"schema_version":1,"profile":profile,"validation":validation,"resources":resources,"bindings":bindings,"origin_check":origin_check,"inventory_diagnostics":inventory.diagnostics,"reception":"not_tested","applied":false}),
                    )
                }
                Command::ProfilesList {} | Command::ProfilesDelete { .. } => {
                    tokio::task::spawn_blocking(move || {
                        let store = profiles::Store::open(&directory)?;
                        match command {
                            Command::ProfilesList {} => store.list(),
                            Command::ProfilesDelete { id } => store.delete(&id),
                            _ => unreachable!(),
                        }
                    })
                    .await
                    .map_err(|_| ApiError::new(ErrorCode::Internal, "profile task failed"))?
                    .map_err(error)
                }
                _ => Err(ApiError::new(
                    ErrorCode::InvalidRequest,
                    "not a profile command",
                )),
            }
        })
    }
    fn export_recordings(&self, destination: String) -> BackendResult {
        self.history(move |reader| reader.export(Path::new(&destination)))
    }
    fn capture_enabled(&self) -> bool {
        self.capture.is_some()
    }
    fn capture(&self, action: airhound_api::CaptureAction) -> BackendResult {
        let handle = self.capture.clone();
        Box::pin(async move {
            use crate::capture_service::Action;
            use airhound_api::CaptureAction;
            let handle = handle.ok_or_else(|| {
                ApiError::new(
                    ErrorCode::UnsupportedMethod,
                    "capture control is not enabled",
                )
            })?;
            let action = match action {
                CaptureAction::Start { request_id, config } => Action::Start { request_id, config },
                CaptureAction::Pause { run_id } => Action::Pause(run_id),
                CaptureAction::Resume { run_id } => Action::Resume(run_id),
                CaptureAction::Finish { run_id } => Action::Finish(run_id),
                CaptureAction::Status => Action::Status,
            };
            handle.request(action).await.map_err(|error| {
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
    fn identify_device(&self, config: Value) -> BackendResult {
        let capture = self.capture.clone();
        Box::pin(async move {
            let request = serde_json::from_value(config)
                .map_err(|error| ApiError::new(ErrorCode::InvalidRequest, error.to_string()))?;
            let capture = capture.ok_or_else(|| ApiError::new(ErrorCode::UnsupportedMethod, "identification requires a capture-capable service to coordinate device ownership"))?;
            capture
                .request(crate::capture_service::Action::Identify(request))
                .await
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
        })
    }
    fn devices(&self, all: bool) -> BackendResult {
        let capture = self.capture.clone();
        Box::pin(async move {
            let mut inventory = airhound_hostd::discovery::discover(
                &airhound_hostd::managed_helpers::discovery_options(all),
            )
            .await;
            if let Some(capture) = capture {
                match capture
                    .request(crate::capture_service::Action::Status)
                    .await
                {
                    Ok(status) => inventory.reuse_active_metadata(&status),
                    Err(error) => {
                        inventory
                            .diagnostics
                            .push(airhound_hostd::discovery::Diagnostic {
                                backend: "active_receiver_metadata".into(),
                                message: error.to_string(),
                            })
                    }
                }
            }
            serde_json::to_value(inventory)
                .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))
        })
    }
    fn validate(&self, config: Value) -> BackendResult {
        Box::pin(async move {
            let plan = airhound_hostd::session::SessionConfig::from_json(config)
                .map_err(|e| ApiError::new(ErrorCode::InvalidSession, e.to_string()))?;
            crate::session_runtime::preview(plan)
                .await
                .map_err(|e| ApiError::new(ErrorCode::InvalidSession, e.to_string()))
        })
    }
}

pub(crate) async fn serve(path: &Path, database: Option<PathBuf>, capture: bool) -> io::Result<()> {
    if capture {
        let path = database
            .clone()
            .ok_or_else(|| io::Error::other("capture requires a database"))?;
        // Recover before advertising capture control; reject a competing writer.
        // History-only services never create or recover recordings.
        tokio::task::spawn_blocking(move || airhound_hostd::storage::Store::open(&path).map(drop))
            .await
            .map_err(io::Error::other)??;
    }
    let actor = if capture {
        Some(crate::capture_service::spawn(database.clone().ok_or_else(
            || io::Error::other("capture requires a database"),
        )?))
    } else {
        None
    };
    let handle = actor.as_ref().map(|(handle, _)| handle.clone());
    let result = airhound_hostd::service::serve(
        path,
        Arc::new(LocalBackend {
            database,
            capture: handle,
            profiles: Some(
                path.parent()
                    .ok_or_else(|| io::Error::other("service socket needs a parent"))?
                    .join("profiles"),
            ),
        }),
        async {
            use tokio::signal::unix::{signal, SignalKind};
            match signal(SignalKind::terminate()) {
                Ok(mut terminate) => {
                    tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
                }
                Err(_) => {
                    let _ = tokio::signal::ctrl_c().await;
                }
            }
        },
    )
    .await;
    if let Some((handle, task)) = actor {
        let stopped = handle
            .request(crate::capture_service::Action::Shutdown)
            .await;
        drop(handle);
        task.await.map_err(io::Error::other)??;
        stopped?;
    }
    result
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
    if let ClientCommand::WaitReady { timeout_ms } = command {
        let status =
            airhound_client::readiness::wait_for_ready(path, Duration::from_millis(timeout_ms))
                .await
                .map_err(io::Error::other)?;
        return crate::observation::write_json(&mut io::stdout(), &status);
    }
    let mut client = Client::connect(path, Duration::from_secs(20))
        .await
        .map_err(io::Error::other)?;
    let result = match command {
        ClientCommand::Profiles { action } => match action {
            ProfileCommand::List => client.list_profiles().await,
            ProfileCommand::Save {
                id,
                name,
                session_config,
                replace,
            } => {
                client
                    .save_profile(id, name, read_config(&session_config)?, replace)
                    .await
            }
            ProfileCommand::Load { id } => client.load_profile(id).await,
            ProfileCommand::Delete { id } => client.delete_profile(id).await,
        },
        ClientCommand::Export { output } => client.export_recordings(output).await,
        ClientCommand::Start {
            session_config,
            request_id,
        } => {
            client
                .capture(airhound_api::CaptureAction::Start {
                    request_id,
                    config: read_config(&session_config)?,
                })
                .await
        }
        ClientCommand::Pause { run_id } => {
            client
                .capture(airhound_api::CaptureAction::Pause { run_id })
                .await
        }
        ClientCommand::Resume { run_id } => {
            client
                .capture(airhound_api::CaptureAction::Resume { run_id })
                .await
        }
        ClientCommand::Finish { run_id } => {
            client
                .capture(airhound_api::CaptureAction::Finish { run_id })
                .await
        }
        ClientCommand::CaptureStatus => client.capture(airhound_api::CaptureAction::Status).await,
        ClientCommand::Status => client.status().await,
        ClientCommand::WaitReady { .. } => unreachable!("handled before connecting"),
        ClientCommand::Recordings { before, limit } => client.recordings(before, limit).await,
        ClientCommand::Records {
            run_id,
            after,
            limit,
        } => client.recording_records(run_id, after, limit).await,
        ClientCommand::Devices { all } => client.devices(all).await,
        ClientCommand::Identify { config } => client.identify_device(read_config(&config)?).await,
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
    #[test]
    fn readiness_cli_rejects_invalid_budgets_before_io() {
        use clap::Parser;
        #[derive(Parser)]
        struct Arguments {
            #[command(subcommand)]
            command: ClientCommand,
        }
        for milliseconds in ["0", "60001", "-1", "invalid"] {
            assert!(Arguments::try_parse_from([
                "fixture",
                "wait-ready",
                "--timeout-ms",
                milliseconds
            ])
            .is_err());
        }
        for milliseconds in [1, 5000, 60000] {
            let parsed = Arguments::try_parse_from([
                "fixture",
                "wait-ready",
                "--timeout-ms",
                &milliseconds.to_string(),
            ])
            .unwrap();
            assert!(
                matches!(parsed.command, ClientCommand::WaitReady { timeout_ms } if timeout_ms == milliseconds)
            );
        }
    }

    #[tokio::test]
    async fn profile_load_returns_editable_configuration_after_serial_alias_collision() {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let root = std::env::temp_dir().join(format!(
            "ah-profile-load-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        for name in ["one", "two", "python", "helper"] {
            std::fs::write(root.join(name), b"never execute this fixture").unwrap();
        }
        std::fs::set_permissions(root.join("python"), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(root.join("two"), &alias).unwrap();
        let source = |port: PathBuf| serde_json::json!({"kind":"sniffle","port":port,"python":root.join("python"),"helper":root.join("helper")});
        let config = serde_json::json!({"version":1,"receivers":[{"id":"one","source":source(root.join("one"))},{"id":"two","source":source(alias.clone())}]});
        let inventory = airhound_hostd::discovery::Inventory {
            schema_version: 1,
            mode: "passive",
            platform: std::env::consts::OS.into(),
            devices: vec![],
            diagnostics: vec![],
        };
        let store = airhound_hostd::profiles::Store::open(&root.join("profiles")).unwrap();
        store
            .save(
                "fixture".into(),
                "Fixture".into(),
                config.clone(),
                &inventory,
                false,
            )
            .unwrap();
        drop(store);
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(root.join("one"), &alias).unwrap();
        let backend = LocalBackend {
            profiles: Some(root.join("profiles")),
            ..Default::default()
        };
        let result = backend
            .profiles(airhound_api::Command::ProfilesLoad {
                id: "fixture".into(),
            })
            .await
            .unwrap();
        assert_eq!(result["profile"]["config"], config);
        assert_eq!(result["validation"]["valid"], false);
        assert_eq!(result["applied"], false);
        assert_eq!(result["origin_check"]["state"], "same_store");
    }
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
