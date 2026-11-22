//! Service-owned capture actor. Request cancellation never cancels a receiver.
use crate::{control::ScanControl, session_runtime, FilterConfig, WigleWriter};
use airhound_hostd::{
    session::{SessionConfig, Source},
    storage::{self, output::Completion, writer::Writer, RecordingMode},
};
use serde_json::{json, Value};
use std::{io, path::PathBuf, sync::atomic::Ordering};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

pub(crate) enum Action {
    Start { request_id: String, config: Value },
    Pause(String),
    Resume(String),
    Finish(String),
    Status,
    Shutdown,
}
struct Request {
    action: Action,
    reply: oneshot::Sender<io::Result<Value>>,
}
#[derive(Clone)]
pub(crate) struct Handle(mpsc::Sender<Request>);
impl Handle {
    pub async fn request(&self, action: Action) -> io::Result<Value> {
        let (reply, receiver) = oneshot::channel();
        self.0
            .send(Request { action, reply })
            .await
            .map_err(|_| io::Error::other("capture service stopped"))?;
        receiver
            .await
            .map_err(|_| io::Error::other("capture request was not acknowledged"))?
    }
}
pub(crate) fn spawn(database: PathBuf) -> (Handle, JoinHandle<io::Result<()>>) {
    let (tx, mut rx) = mpsc::channel::<Request>(8);
    let task = tokio::spawn(async move {
        let mut active: Option<Active> = None;
        let mut last: Option<Value> = None;
        while let Some(request) = rx.recv().await {
            if active.as_ref().is_some_and(|run| run.task.is_finished()) {
                last = Some(active.take().expect("active run").complete().await);
            }
            let shutdown = matches!(request.action, Action::Shutdown);
            let paused = matches!(request.action, Action::Pause(_));
            let result = match request.action {
                Action::Start { request_id, config } => {
                    let path = database.clone();
                    let key = request_id.clone();
                    let original = config.clone();
                    let receipt = tokio::task::spawn_blocking(move || {
                        if key.is_empty() || key.len() > 128 || key.chars().any(char::is_control) {
                            return Err(invalid(
                                "start request id must contain 1–128 non-control bytes",
                            ));
                        }
                        match storage::Reader::open(&path) {
                            Ok(reader) => reader.start_receipt(&key, &original),
                            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                            Err(error) => Err(error),
                        }
                    })
                    .await
                    .map_err(io::Error::other)
                    .and_then(|result| result);
                    if let Ok(Some(mut receipt)) = receipt {
                        receipt["active"] = json!(active
                            .as_ref()
                            .is_some_and(|run| receipt["run_id"] == run.id));
                        Ok(receipt)
                    } else if let Err(error) = receipt {
                        Err(error)
                    } else if active.is_some() {
                        Err(invalid("a recording is already active"))
                    } else {
                        match start(database.clone(), request_id, config).await {
                            Ok(run) => {
                                let status = run.status();
                                active = Some(run);
                                Ok(status)
                            }
                            Err(error) => Err(error),
                        }
                    }
                }
                Action::Status => Ok(active
                    .as_ref()
                    .map(Active::status)
                    .unwrap_or_else(|| json!({"active":false,"last":last}))),
                Action::Pause(id) | Action::Resume(id) => {
                    match active.as_ref().filter(|run| run.id == id) {
                        Some(run) => run.transition(paused).await,
                        None => Err(invalid("recording is not active in this service")),
                    }
                }
                Action::Finish(id) => {
                    if active.as_ref().is_some_and(|run| run.id == id) {
                        let run = active.take().expect("active run");
                        run.control.shutdown();
                        let result = run.complete().await;
                        last = Some(result.clone());
                        Ok(result)
                    } else if last.as_ref().is_some_and(|run| run["run_id"] == id) {
                        Ok(last.clone().expect("last run"))
                    } else {
                        Err(invalid("recording is not active in this service"))
                    }
                }
                Action::Shutdown => {
                    if let Some(run) = active.take() {
                        run.control.shutdown();
                        last = Some(run.complete().await);
                    }
                    Ok(json!({"active":false,"last":last}))
                }
            };
            let _ = request.reply.send(result);
            if shutdown {
                if last.as_ref().is_some_and(|run| run["state"] == "failed") {
                    return Err(io::Error::other(
                        "capture ended with a failure; inspect recording history",
                    ));
                }
                return Ok(());
            }
        }
        if let Some(run) = active {
            run.control.shutdown();
            let result = run.complete().await;
            if result["state"] != "finished" {
                return Err(io::Error::other("capture cleanup failed"));
            }
        }
        Ok(())
    });
    (Handle(tx), task)
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
struct Active {
    id: String,
    control: ScanControl,
    recording: Completion,
    task: JoinHandle<io::Result<()>>,
}
impl Active {
    fn status(&self) -> Value {
        json!({"active":true,"run_id":self.id,"state":if self.control.load(Ordering::Relaxed) { "recording" } else { "paused" },"receiver_health":self.control.health_message()})
    }
    async fn transition(&self, paused: bool) -> io::Result<Value> {
        if self.control.load(Ordering::Relaxed) != !paused {
            if paused {
                self.control.store(false, Ordering::Relaxed);
            }
            if let Err(error) = self.recording.transition(paused).await {
                self.control.shutdown();
                return Err(error);
            }
            if !paused {
                self.control.store(true, Ordering::Relaxed);
            }
        }
        Ok(self.status())
    }
    async fn complete(self) -> Value {
        let result = self
            .task
            .await
            .map_err(io::Error::other)
            .and_then(|result| result);
        match result {
            Ok(()) => json!({"active":false,"run_id":self.id,"state":"finished"}),
            Err(error) => {
                json!({"active":false,"run_id":self.id,"state":"failed","error":error.to_string()})
            }
        }
    }
}
async fn start(database: PathBuf, request_id: String, config: Value) -> io::Result<Active> {
    if serde_json::to_vec(&config).map_err(io::Error::other)?.len() > airhound_api::MAX_CONFIG_BYTES
    {
        return Err(invalid("session config exceeds 64 KiB"));
    }
    let plan: SessionConfig = serde_json::from_value(config.clone()).map_err(io::Error::other)?;
    let request = (request_id, config);
    let plan = plan.resolve()?;
    let mut filter = FilterConfig::new();
    if let Some(policy) = plan.alert_policy {
        filter.alert_policy = policy;
    }
    let config = json!({"session":plan,"alert_policy":filter.alert_policy});
    let replay = plan.receivers.iter().all(|receiver| {
        matches!(
            receiver.source,
            Source::BlePcap { .. } | Source::WifiPcap { .. }
        )
    });
    let mut sources: Vec<_> = plan
        .receivers
        .iter()
        .map(|receiver| {
            (
                receiver.id.clone(),
                json!({"requested_source":receiver.source}),
            )
        })
        .collect();
    sources.push((
        storage::output::SESSION_SOURCE.into(),
        json!({"kind":"session_control"}),
    ));
    let receivers = session_runtime::prepare(plan).await?;
    let writer = tokio::task::spawn_blocking(move || {
        Writer::start_request(
            database,
            if replay {
                RecordingMode::Replay
            } else {
                RecordingMode::Live
            },
            config,
            sources,
            Some(request),
        )
    })
    .await
    .map_err(io::Error::other)??;
    let id = writer.run_id.clone();
    let (output, recording) = storage::output::attach(io::sink(), writer);
    let control = ScanControl::new(true);
    let capture_control = control.clone();
    let mut stop = control.subscribe();
    let completion = recording.clone();
    let task = tokio::spawn(async move {
        let csv = WigleWriter::new(io::sink())?;
        let result = session_runtime::run_controlled(
            receivers,
            (filter, capture_control),
            csv,
            output,
            None,
            |_, _, _| async {},
            async move {
                loop {
                    if stop.borrow().shutdown {
                        return Ok(());
                    }
                    stop.changed().await.map_err(io::Error::other)?;
                }
            },
        )
        .await;
        let succeeded = result.is_ok();
        tokio::task::spawn_blocking(move || completion.finish(succeeded))
            .await
            .map_err(io::Error::other)??;
        result
    });
    Ok(Active {
        id,
        control,
        recording,
        task,
    })
}
