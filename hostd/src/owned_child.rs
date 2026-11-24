//! One supervisor owns child cancellation and reaping, even if its caller is
//! cancelled. Tokio's kill-on-drop remains the fallback when the runtime stops.
use std::{io, process::ExitStatus, time::Duration};
use tokio::{
    process::{Child, ChildStdout},
    sync::oneshot,
    task::JoinHandle,
};

pub(crate) struct OwnedChild {
    pub stdout: Option<ChildStdout>,
    stop: Option<oneshot::Sender<()>>,
    worker: Option<JoinHandle<io::Result<ExitStatus>>>,
}

#[cfg(test)]
pub(crate) type Reaped = tokio::sync::mpsc::Sender<(u32, bool)>;

impl OwnedChild {
    /// The supplied command must set kill_on_drop(true) before spawning.
    /// No await separates child ownership from registration of its supervisor.
    pub fn new(mut child: Child, #[cfg(test)] reaped: Option<Reaped>) -> io::Result<Self> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| io::Error::other("child supervisor needs a running runtime"))?;
        #[cfg(test)]
        let pid = child.id().expect("new child has not been reaped");
        let stdout = child.stdout.take();
        let (stop, cancelled) = oneshot::channel();
        let worker = runtime.spawn(async move {
            let (result, _reap_confirmed) = tokio::select! {
                status = child.wait() => { let confirmed = status.is_ok(); (status, confirmed) },
                _ = cancelled => reap_after_kill(child.start_kill(), child.wait(), #[cfg(test)] None).await,
            };
            #[cfg(test)]
            if let Some(witness) = reaped {
                let _ = witness.try_send((pid, _reap_confirmed));
            }
            result
        });
        Ok(Self {
            stdout,
            stop: Some(stop),
            worker: Some(worker),
        })
    }

    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        let result = self
            .worker
            .as_mut()
            .expect("supervisor consumed once")
            .await
            .map_err(|_| io::Error::other("child supervisor failed"))?;
        self.worker.take();
        result
    }
    pub async fn finish(mut self) -> io::Result<ExitStatus> {
        self.stop.take();
        self.wait().await
    }
}

/// Always attempt wait, even when termination reports an already-exited child
/// or another error. Retain both errors while distinguishing proven reaping.
async fn reap_after_kill(
    killed: io::Result<()>,
    wait: impl std::future::Future<Output = io::Result<ExitStatus>>,
    #[cfg(test)] witness: Option<crate::test_support::TimerWitness>,
) -> (io::Result<ExitStatus>, bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let waiting = tokio::time::timeout_at(deadline, wait);
    #[cfg(test)]
    let waiting = async {
        if let Some(witness) = witness {
            witness.observe(deadline, waiting, Result::is_err).await
        } else {
            waiting.await
        }
    };
    let waited = waiting
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "child reap deadline"))
        .and_then(|result| result);
    let reaped = waited.is_ok();
    let result = match (killed, waited) {
        (Ok(()), result) => result,
        (Err(kill), Ok(_)) => Err(io::Error::new(
            kill.kind(),
            format!("child reaped after termination error: {kill}"),
        )),
        (Err(kill), Err(wait)) => Err(io::Error::new(
            wait.kind(),
            format!("child termination error: {kill}; reap error: {wait}"),
        )),
    };
    (result, reaped)
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        // Dropping a JoinHandle detaches the worker; it retains the exact child
        // until wait completes. Dropping stop requests cancellation once.
        self.stop.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;
    use tokio::{
        io::{AsyncBufReadExt, BufReader},
        process::Command,
        sync::mpsc,
    };

    async fn child(script: &str) -> (OwnedChild, mpsc::Receiver<(u32, bool)>) {
        let process = Command::new("python3")
            .args(["-c", script])
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let (tx, rx) = mpsc::channel(1);
        (OwnedChild::new(process, Some(tx)).unwrap(), rx)
    }
    async fn assert_reaped(rx: &mut mpsc::Receiver<(u32, bool)>) {
        let (pid, reaped) = tokio::time::timeout(Duration::from_secs(4), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(reaped);
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }
    #[tokio::test]
    async fn natural_exit_is_reaped_and_exit_failure_is_preserved() {
        crate::test_support::manual_time(async {
            let (owned, mut witness) = child("raise SystemExit(7)").await;
            assert_reaped(&mut witness).await;
            assert_eq!(owned.finish().await.unwrap().code(), Some(7));
        })
        .await;
    }
    #[tokio::test]
    async fn caller_cancellation_uses_the_same_owned_reaper() {
        crate::test_support::manual_time(async {
            let (mut owned, mut witness) =
                child("import os,signal;print(os.getpid(),flush=True);signal.pause()").await;
            let mut line = String::new();
            BufReader::new(owned.stdout.take().unwrap())
                .read_line(&mut line)
                .await
                .unwrap();
            let task = tokio::spawn(async move {
                let _owned = owned;
                std::future::pending::<()>().await;
            });
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            assert_reaped(&mut witness).await;
        })
        .await;
    }
    #[tokio::test]
    async fn termination_error_still_waits_for_the_real_child() {
        crate::test_support::manual_time(async {
            let mut process = Command::new("python3")
                .args(["-c", "raise SystemExit(0)"])
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let pid = process.id().unwrap();
            let (result, reaped) = reap_after_kill(
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected kill rejection",
                )),
                process.wait(),
                None,
            )
            .await;
            assert!(reaped);
            let error = result.unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert!(error.to_string().contains("reaped after termination error"));
            assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        })
        .await;
    }
    #[tokio::test]
    async fn termination_and_wait_errors_both_survive() {
        crate::test_support::manual_time(async {
            let (result, reaped) = reap_after_kill(
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "kill rejected",
                )),
                async { Err(io::Error::other("wait rejected")) },
                None,
            )
            .await;
            assert!(!reaped);
            let message = result.unwrap_err().to_string();
            assert!(message.contains("kill rejected") && message.contains("wait rejected"));
        })
        .await;
    }
    #[tokio::test]
    async fn pending_reap_reports_virtual_timeout_and_retains_kill_error() {
        crate::test_support::manual_time(async {
            for kill_fails in [false, true] {
                let killed = if kill_fails {
                    Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "kill rejected",
                    ))
                } else {
                    Ok(())
                };
                let (witness, mut events) = crate::test_support::TimerWitness::channel();
                let task = tokio::spawn(reap_after_kill(
                    killed,
                    std::future::pending::<io::Result<ExitStatus>>(),
                    Some(witness),
                ));
                crate::test_support::advance_registered_timer(&mut events).await;
                let (result, reaped) = task.await.unwrap();
                assert!(!reaped);
                let error = result.unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::TimedOut);
                assert!(error.to_string().contains("child reap deadline"));
                assert_eq!(error.to_string().contains("kill rejected"), kill_fails);
            }
        })
        .await;
    }
}
