//! Read-only startup readiness: one deadline for connect, authenticated Hello,
//! Status and retries. Existing sockets and running services are never modified.
use crate::{Client, ClientError};
use serde_json::Value;
use std::{future::Future, io, path::Path, time::Duration};
use tokio::time::{sleep_until, timeout_at, Instant};

pub const MAX_WAIT: Duration = Duration::from_secs(60);
const RETRY_DELAY: Duration = Duration::from_millis(100);

pub(super) struct AttemptFailure {
    pub(super) error: ClientError,
    retryable_connect: bool,
}

impl AttemptFailure {
    pub(super) fn connection(error: io::Error) -> Self {
        let retryable_connect = matches!(
            error.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
        );
        Self {
            error: ClientError::Io(error),
            retryable_connect,
        }
    }

    pub(super) fn terminal(error: ClientError) -> Self {
        Self {
            error,
            retryable_connect: false,
        }
    }
}

/// Return the existing Status response after the service has authenticated and
/// answered. Only an absent/refused transport connection can be retried. Once a
/// connection exists, permission, handshake, protocol and request errors retain
/// their identity and terminate this operation.
pub async fn wait_for_ready(path: &Path, budget: Duration) -> Result<Value, ClientError> {
    wait_with(budget, |remaining| async move {
        let mut client = Client::connect_for_readiness(path, remaining).await?;
        client.status().await.map_err(AttemptFailure::terminal)
    })
    .await
}

async fn wait_with<F, Fut>(budget: Duration, mut attempt: F) -> Result<Value, ClientError>
where
    F: FnMut(Duration) -> Fut,
    Fut: Future<Output = Result<Value, AttemptFailure>>,
{
    if budget.is_zero() || budget > MAX_WAIT {
        return Err(ClientError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "readiness wait must be greater than zero and at most 60 seconds",
        )));
    }
    let deadline = Instant::now() + budget;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(ClientError::Timeout);
        }
        let result = timeout_at(deadline, attempt(deadline - now))
            .await
            .map_err(|_| ClientError::Timeout)?;
        // A ready final poll at the deadline must not bypass the total budget.
        if Instant::now() >= deadline {
            return Err(ClientError::Timeout);
        }
        match result {
            Ok(status) => return Ok(status),
            Err(failure) if failure.retryable_connect => {
                sleep_until((Instant::now() + RETRY_DELAY).min(deadline)).await;
            }
            Err(failure) => return Err(failure.error),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::{cell::RefCell, future::poll_fn, pin::Pin, rc::Rc, task::Poll};

    async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
        poll_fn(|context| Poll::Ready(future.as_mut().poll(context))).await
    }

    struct SocketFixture {
        root: std::path::PathBuf,
    }
    impl SocketFixture {
        fn new() -> Self {
            use std::os::unix::fs::DirBuilderExt;
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let root = loop {
                let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let root =
                    std::env::temp_dir().join(format!("ah-ready-{}-{id}", std::process::id()));
                match std::fs::DirBuilder::new().mode(0o700).create(&root) {
                    Ok(()) => break root,
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(e) => panic!("private socket fixture: {e}"),
                }
            };
            Self { root }
        }
    }
    impl Drop for SocketFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    async fn hello(peer: &mut tokio::net::UnixStream) {
        use crate::{read_frame, write_frame};
        use airhound_api::{Command, Hello, Request, Response, MAX_FRAME_BYTES, VERSION};
        let request: Request = serde_json::from_slice(&read_frame(peer).await.unwrap()).unwrap();
        assert!(matches!(request.command, Command::Hello {}));
        write_frame(
            peer,
            &Response::success(
                request.request_id,
                serde_json::to_value(Hello {
                    version: VERSION,
                    instance_id: "readiness-fixture".into(),
                    build_version: "fixture".into(),
                    max_frame_bytes: MAX_FRAME_BYTES,
                    methods: vec!["status".into()],
                    mode: "passive_control".into(),
                })
                .unwrap(),
            ),
        )
        .await
        .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn actual_socket_hello_status_and_cancellation_close_only_owned_connection() {
        use crate::{read_frame, write_frame};
        use airhound_api::{Command, Request, Response};
        use tokio::io::AsyncReadExt;
        let clock = crate::test_clock::ManualTime::hold().await;
        let fixture = SocketFixture::new();
        let path = fixture.root.join("control.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let expected = serde_json::json!({"mode":"passive_control", "capture_control":false});
        let server = async {
            let (mut peer, _) = listener.accept().await.unwrap();
            hello(&mut peer).await;
            let request: Request =
                serde_json::from_slice(&read_frame(&mut peer).await.unwrap()).unwrap();
            assert!(matches!(request.command, Command::Status {}));
            write_frame(
                &mut peer,
                &Response::success(request.request_id, expected.clone()),
            )
            .await
            .unwrap();
            assert_eq!(peer.read(&mut [0]).await.unwrap(), 0);
        };
        let (result, ()) = tokio::join!(wait_for_ready(&path, Duration::from_secs(1)), server);
        assert_eq!(result.unwrap(), expected);
        // The same listener accepts again: readiness neither shuts down nor
        // unlinks the service. Cancel while the real Status response is pending.
        let (seen, requested) = tokio::sync::oneshot::channel();
        let (closed, eof) = tokio::sync::oneshot::channel();
        let server = async {
            let (mut peer, _) = listener.accept().await.unwrap();
            hello(&mut peer).await;
            let request: Request =
                serde_json::from_slice(&read_frame(&mut peer).await.unwrap()).unwrap();
            assert!(matches!(request.command, Command::Status {}));
            seen.send(()).unwrap();
            assert_eq!(peer.read(&mut [0]).await.unwrap(), 0);
            closed.send(()).unwrap();
        };
        let client = async {
            let mut wait = Box::pin(wait_for_ready(&path, Duration::from_secs(1)));
            tokio::select! { result = &mut wait => panic!("unexpected readiness: {result:?}"), _ = requested => {} }
            drop(wait);
            eof.await.unwrap();
        };
        tokio::join!(server, client);
        assert!(path.exists());
        drop(listener);
        drop(fixture);
        clock.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn retries_only_after_explicit_delay_with_one_decreasing_budget() {
        let budgets = Rc::new(RefCell::new(Vec::new()));
        let observed = budgets.clone();
        let wait = wait_with(Duration::from_millis(500), move |remaining| {
            observed.borrow_mut().push(remaining);
            let attempt = observed.borrow().len();
            async move {
                if attempt == 3 {
                    Ok(serde_json::json!({"capture_control": true}))
                } else {
                    Err(AttemptFailure::connection(io::Error::from(
                        if attempt == 1 {
                            io::ErrorKind::NotFound
                        } else {
                            io::ErrorKind::ConnectionRefused
                        },
                    )))
                }
            }
        });
        tokio::pin!(wait);
        assert!(poll_once(wait.as_mut()).await.is_pending());
        tokio::time::advance(Duration::from_millis(99)).await;
        assert!(poll_once(wait.as_mut()).await.is_pending());
        assert_eq!(budgets.borrow().len(), 1);
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(poll_once(wait.as_mut()).await.is_pending());
        assert_eq!(budgets.borrow().len(), 2);
        tokio::time::advance(Duration::from_millis(100)).await;
        assert!(matches!(poll_once(wait.as_mut()).await, Poll::Ready(Ok(_))));
        assert_eq!(
            *budgets.borrow(),
            [500, 400, 300].map(Duration::from_millis)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn pending_attempt_expires_at_total_deadline_and_is_dropped() {
        struct Dropped(Rc<RefCell<bool>>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                *self.0.borrow_mut() = true;
            }
        }
        let dropped = Rc::new(RefCell::new(false));
        let marker = dropped.clone();
        let wait = wait_with(Duration::from_secs(1), move |_| {
            let marker = marker.clone();
            async move {
                let _owned = Dropped(marker);
                std::future::pending().await
            }
        });
        tokio::pin!(wait);
        assert!(poll_once(wait.as_mut()).await.is_pending());
        tokio::time::advance(Duration::from_millis(999)).await;
        assert!(poll_once(wait.as_mut()).await.is_pending());
        assert!(!*dropped.borrow());
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(matches!(
            poll_once(wait.as_mut()).await,
            Poll::Ready(Err(ClientError::Timeout))
        ));
        assert!(*dropped.borrow());
    }

    #[tokio::test(start_paused = true)]
    async fn final_response_at_or_after_deadline_is_not_accepted() {
        for elapsed in [1000, 1001] {
            let wait = wait_with(Duration::from_secs(1), |_| async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok(Value::Null)
            });
            tokio::pin!(wait);
            assert!(poll_once(wait.as_mut()).await.is_pending());
            tokio::time::advance(Duration::from_millis(elapsed)).await;
            assert!(matches!(
                poll_once(wait.as_mut()).await,
                Poll::Ready(Err(ClientError::Timeout))
            ));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn protocol_permission_and_post_connect_io_errors_are_terminal() {
        for failure in [
            AttemptFailure::connection(io::Error::from(io::ErrorKind::PermissionDenied)),
            AttemptFailure::terminal(ClientError::Io(io::Error::from(io::ErrorKind::NotFound))),
            AttemptFailure::terminal(ClientError::Io(io::Error::from(
                io::ErrorKind::ConnectionRefused,
            ))),
            AttemptFailure::terminal(ClientError::Protocol("wrong hello".into())),
            AttemptFailure::terminal(ClientError::Remote(airhound_api::ApiError::new(
                airhound_api::ErrorCode::UnsupportedMethod,
                "status unsupported",
            ))),
        ] {
            let expected = failure.error.to_string();
            let mut pending = Some(failure);
            let result = wait_with(Duration::from_secs(1), |_| {
                std::future::ready(Err(pending
                    .take()
                    .expect("must not retry terminal failure")))
            })
            .await;
            assert_eq!(result.unwrap_err().to_string(), expected);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn zero_and_excessive_budgets_never_attempt_a_connection() {
        for budget in [Duration::ZERO, MAX_WAIT + Duration::from_millis(1)] {
            let result =
                wait_with(budget, |_| async { panic!("invalid budget attempted IO") }).await;
            assert!(
                matches!(result, Err(ClientError::Io(error)) if error.kind() == io::ErrorKind::InvalidInput)
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_wait_drops_inflight_operation_without_another_attempt() {
        struct Owned(Rc<RefCell<u32>>);
        impl Drop for Owned {
            fn drop(&mut self) {
                *self.0.borrow_mut() += 1;
            }
        }
        let dropped = Rc::new(RefCell::new(0));
        let marker = dropped.clone();
        let mut wait = Box::pin(wait_with(Duration::from_secs(1), move |_| {
            let marker = marker.clone();
            async move {
                let _owned = Owned(marker);
                std::future::pending().await
            }
        }));
        assert!(poll_once(wait.as_mut()).await.is_pending());
        assert_eq!(*dropped.borrow(), 0);
        drop(wait);
        assert_eq!(*dropped.borrow(), 1);
    }
}
