//! Continuous legacy BLE reporting without resetting the scan session.

use bt_hci::cmd::le::LeSetScanEnable;
use core::future::Future;

/// Which awaited operation prevented capture readiness.
#[derive(Debug, PartialEq, Eq)]
pub enum StartError<E> {
    Scan(E),
    DuplicateFilter(E),
}

/// Retain the successfully started scan session while disabling the controller's duplicate filter.
///
/// trouble-host 0.6 legacy scanning enables filtering by default. Bluetooth Core
/// Vol 4, Part E, 7.8.11 permits changing it while scanning remains enabled. Use
/// the existing stack command owner; do not stop/restart scanning or change its
/// active/passive mode, interval, or window. The caller must await this result
/// before enabling the rest of capture. Accept the completed scan result rather
/// than its future so the embedded task can reuse that future's storage while
/// awaiting the controller command. Failure/cancellation drops the session,
/// allowing its normal RAII cancellation to disable scanning.
pub async fn start_continuous_scan<S, E, F>(
    scan: Result<S, E>,
    command: impl FnOnce(LeSetScanEnable) -> F,
) -> Result<S, StartError<E>>
where
    F: Future<Output = Result<(), E>>,
{
    let session = scan.map_err(StartError::Scan)?;
    command(LeSetScanEnable::new(true, false))
        .await
        .map_err(StartError::DuplicateFilter)?;
    Ok(session)
}

/// Keep a started scan session alive without scheduling periodic wakeups.
///
/// The future owns the session before its first poll and throughout the pending
/// await. Cancelling/dropping it runs the session's normal RAII teardown exactly
/// once. The explicit use after the await keeps ownership across suspension;
/// no timer, allocation, or separate task is needed.
pub async fn retain_scan_session<S>(session: S) {
    core::future::pending::<()>().await;
    drop(session);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_hci::WriteHci;
    use core::{
        future::poll_fn,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    use std::{cell::Cell, rc::Rc};

    #[derive(Debug)]
    struct Session(Rc<Cell<usize>>);

    impl Drop for Session {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn retained_session_never_schedules_a_wakeup_and_drops_on_cancellation() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        struct Wakes(AtomicUsize);
        impl std::task::Wake for Wakes {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let drops = Rc::new(Cell::new(0));
        let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
        let waker = Waker::from(wakes.clone());
        {
            let operation = retain_scan_session(Session(drops.clone()));
            let mut operation = pin!(operation);
            let mut cx = Context::from_waker(&waker);
            assert!(operation.as_mut().poll(&mut cx).is_pending());
            assert_eq!(drops.get(), 0, "pending scan must retain its session");
            assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
        }
        assert_eq!(drops.get(), 1, "cancellation must release the session once");
        assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn unpolled_retained_session_is_also_owned_and_released() {
        let drops = Rc::new(Cell::new(0));
        let operation = retain_scan_session(Session(drops.clone()));
        assert_eq!(drops.get(), 0);
        drop(operation);
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn readiness_waits_for_scan_then_exact_unfiltered_command_ack() {
        let scan_ready = Cell::new(false);
        let ack_ready = Cell::new(false);
        let commands = Cell::new(0);
        let drops = Rc::new(Cell::new(0));
        let mut session = Some(Session(drops.clone()));
        let scan = poll_fn(|_| {
            if scan_ready.get() {
                Poll::Ready(Ok::<_, &'static str>(session.take().unwrap()))
            } else {
                Poll::Pending
            }
        });
        let operation = async {
            start_continuous_scan(scan.await, |command| {
                commands.set(commands.get() + 1);
                // Actual bt-hci wire encoding: LE Set Scan Enable opcode 0x200c,
                // length 2, scanning enabled, duplicate filtering disabled.
                let mut wire = [0xff; 5];
                command.write_hci(&mut wire[..]).unwrap();
                assert_eq!(wire, [0x0c, 0x20, 0x02, 0x01, 0x00]);
                poll_fn(|_| {
                    if ack_ready.get() {
                        Poll::Ready(Ok(()))
                    } else {
                        Poll::Pending
                    }
                })
            })
            .await
        };
        let mut operation = pin!(operation);
        let mut cx = Context::from_waker(Waker::noop());
        assert!(operation.as_mut().poll(&mut cx).is_pending());
        assert_eq!(commands.get(), 0);
        scan_ready.set(true);
        assert!(operation.as_mut().poll(&mut cx).is_pending());
        assert_eq!(commands.get(), 1);
        assert_eq!(drops.get(), 0);
        ack_ready.set(true);
        let Poll::Ready(Ok(session)) = operation.as_mut().poll(&mut cx) else {
            panic!("capture must become ready only after the command ACK");
        };
        assert_eq!(commands.get(), 1);
        assert_eq!(drops.get(), 0, "successful setup must retain the scan");
        drop(session);
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn scan_failure_sends_no_enable_command() {
        let operation = start_continuous_scan(Err::<Session, _>("scan rejected"), |_| {
            panic!("must not configure a scan that never started");
            #[allow(unreachable_code)]
            async {
                Ok(())
            }
        });
        let mut operation = pin!(operation);
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(
            operation.as_mut().poll(&mut cx),
            Poll::Ready(Err(StartError::Scan("scan rejected")))
        ));
    }

    #[test]
    fn rejected_filter_command_drops_session_without_readiness_or_retry() {
        let drops = Rc::new(Cell::new(0));
        let commands = Cell::new(0);
        let operation = start_continuous_scan(Ok(Session(drops.clone())), |_| {
            commands.set(commands.get() + 1);
            async { Err("command rejected") }
        });
        let mut operation = pin!(operation);
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(
            operation.as_mut().poll(&mut cx),
            Poll::Ready(Err(StartError::DuplicateFilter("command rejected")))
        ));
        assert_eq!(commands.get(), 1);
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn cancellation_while_waiting_for_ack_drops_session() {
        let drops = Rc::new(Cell::new(0));
        {
            let operation = start_continuous_scan(Ok::<_, ()>(Session(drops.clone())), |_| {
                core::future::pending()
            });
            let mut operation = pin!(operation);
            let mut cx = Context::from_waker(Waker::noop());
            assert!(operation.as_mut().poll(&mut cx).is_pending());
            assert_eq!(drops.get(), 0);
        }
        assert_eq!(drops.get(), 1);
    }
}
