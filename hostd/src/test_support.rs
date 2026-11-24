//! Test-only causal timer observations. Real-time watchdogs may bound test
//! failure; behavior assertions advance a registered Tokio deadline explicitly.
use std::{future::Future, task::Poll, time::Duration};
use tokio::{sync::mpsc, time::Instant};

/// Tokio documents that blocking work inhibits paused-clock auto-advance. A
/// blocking channel wait supplies that inhibition without spinning while fake
/// subprocess/socket I/O completes. Its real-time limit is only a test-failure
/// watchdog; no successful behavior depends on real elapsed time.
pub(crate) async fn manual_time<F: Future>(future: F) -> F::Output {
    struct Resume(Option<std::sync::mpsc::Sender<()>>);
    impl Drop for Resume {
        fn drop(&mut self) {
            if let Some(release) = self.0.take() {
                let _ = release.send(());
            }
            tokio::time::resume();
        }
    }
    tokio::time::pause();
    let (release, wait) = std::sync::mpsc::channel();
    let mut guard = Resume(Some(release));
    let mut watchdog =
        tokio::task::spawn_blocking(move || wait.recv_timeout(Duration::from_secs(30)).is_err());
    let result = tokio::select! {
        biased;
        ended = &mut watchdog => panic!("test I/O watchdog expired: {ended:?}"),
        result = future => result,
    };
    guard.0.take().unwrap().send(()).unwrap();
    assert!(!watchdog.await.unwrap(), "test exceeded failure watchdog");
    result
}

#[derive(Clone)]
pub(crate) struct TimerWitness {
    events: mpsc::UnboundedSender<TimerEvent>,
}
#[derive(Debug)]
pub(crate) enum TimerEvent {
    Registered(Instant, std::task::Waker),
    Pending(Instant),
    Fired,
}
impl TimerWitness {
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<TimerEvent>) {
        let (events, rx) = mpsc::unbounded_channel();
        (Self { events }, rx)
    }
    pub async fn observe<F: Future>(
        self,
        deadline: Instant,
        future: F,
        fired: impl Fn(&F::Output) -> bool,
    ) -> F::Output {
        tokio::pin!(future);
        let mut registered = false;
        std::future::poll_fn(|cx| {
            let result = future.as_mut().poll(cx);
            match &result {
                Poll::Pending => {
                    if !registered {
                        registered = true;
                        let _ = self
                            .events
                            .send(TimerEvent::Registered(deadline, cx.waker().clone()));
                    }
                    let _ = self.events.send(TimerEvent::Pending(Instant::now()));
                }
                Poll::Ready(value) if fired(value) => {
                    let _ = self.events.send(TimerEvent::Fired);
                }
                _ => {}
            }
            result
        })
        .await
    }
}

pub(crate) async fn advance_registered_timer(rx: &mut mpsc::UnboundedReceiver<TimerEvent>) {
    let Some(TimerEvent::Registered(deadline, waker)) = rx.recv().await else {
        panic!("expected registered deadline before time advancement")
    };
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .expect("manual clock must precede registered deadline");
    assert!(!remaining.is_zero());
    // Tokio's timer wheel has millisecond granularity. Assert before the
    // logical deadline, then drive through its next supported timer tick.
    // This quantization applies only to the test clock, never the budget.
    assert!(remaining >= Duration::from_millis(1));
    tokio::time::advance(remaining - Duration::from_millis(1)).await;
    let before = Instant::now();
    waker.wake_by_ref();
    loop {
        match rx.recv().await {
            Some(TimerEvent::Pending(observed)) if observed == before => break,
            Some(TimerEvent::Pending(observed)) => assert!(observed < before),
            other => panic!("expected causally observed pending timer before deadline: {other:?}"),
        }
    }
    tokio::time::advance(Duration::from_millis(2)).await;
    loop {
        match rx.recv().await {
            Some(TimerEvent::Fired) => break,
            Some(TimerEvent::Pending(observed)) => assert!(observed <= Instant::now()),
            other => panic!("expected timer completion at next timer tick: {other:?}"),
        }
    }
}

pub(crate) struct ReadWitness<R> {
    pub inner: R,
    pub consumed: Option<tokio::sync::oneshot::Sender<()>>,
}
impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for ReadWitness<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let result = std::pin::Pin::new(&mut self.inner).poll_read(cx, buf);
        if matches!(result, std::task::Poll::Ready(Ok(())))
            && buf.filled()[before..].contains(&b'\n')
        {
            if let Some(witness) = self.consumed.take() {
                let _ = witness.send(());
            }
        }
        result
    }
}
