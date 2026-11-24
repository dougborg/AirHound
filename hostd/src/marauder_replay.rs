//! Explicit saved-file Marauder replay. No radio, serial port, helper or live GPS.
use crate::{
    control::{wait_running, ScanControl},
    health::{Phase, Scanner},
    observation::ScanEvent,
};
use airhound_hostd::{marauder, session::MarauderFormat};
use std::{io, path::PathBuf};
use tokio::{io::AsyncReadExt, sync::mpsc};

// Bound indexing overhead independently of the byte limit for tiny packets.
const MAX_RECORDS: usize = 100_000;

async fn ready(
    state: &mut tokio::sync::watch::Receiver<crate::control::State>,
    tx: &mpsc::Sender<ScanEvent>,
    control: &ScanControl,
) -> bool {
    if !wait_running(state, tx).await {
        return false;
    }
    // Hold the state read lock while publishing health so a concurrent pause
    // cannot be overwritten by a late Running update.
    let current = state.borrow();
    if current.enabled && !current.shutdown {
        control.set_health(Scanner::Wifi, Phase::Running, "marauder_capture_replay");
    }
    true
}

async fn consume(
    path: PathBuf,
    format: MarauderFormat,
    tx: &mpsc::Sender<ScanEvent>,
    control: &ScanControl,
) -> io::Result<(bool, usize)> {
    let mut state = control.subscribe();
    if !wait_running(&mut state, tx).await {
        return Ok((false, 0));
    }
    // Reject a selected device/FIFO/symlink before open: a UART open alone can
    // reset a board. Validate the opened descriptor again against replacements.
    let selected = tokio::fs::symlink_metadata(&path).await?;
    if !selected.is_file() || selected.len() > marauder::MAX_CAPTURE_BYTES as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Choose a regular nonsymlink saved Marauder capture",
        ));
    }
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    let file = options.open(path).await?;
    let metadata = file.metadata().await?;
    if !metadata.is_file() || metadata.len() > marauder::MAX_CAPTURE_BYTES as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Marauder replay requires a regular saved file of at most 64 MiB",
        ));
    }
    let mut bytes = Vec::new();
    let mut bounded = file.take(marauder::MAX_CAPTURE_BYTES as u64 + 1);
    bounded.read_to_end(&mut bytes).await?;
    let mut records = Vec::new();
    marauder::decode(&bytes, matches!(format, MarauderFormat::Serial), |record| {
        if records.len() == MAX_RECORDS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Marauder replay exceeds 100,000 packets",
            ));
        }
        // All slices originate from this immutable buffer; retain bounded offsets,
        // not a second full copy of the input or unbounded decoded JSON values.
        let start = record.frame.as_ptr() as usize - bytes.as_ptr() as usize;
        records.push((record.timestamp_micros32, start..start + record.frame.len()));
        Ok(())
    })?;
    let mut emitted = 0;
    for (timestamp_micros32, range) in records {
        if !ready(&mut state, tx, control).await {
            return Ok((false, emitted));
        }
        let event = ScanEvent::MarauderReplay {
            timestamp_micros32,
            frame: bytes[range].to_vec(),
        };
        // Reserve queue space before committing the packet so pause/stop remain
        // responsive even if the consumer is paused with a full queue.
        loop {
            tokio::select! {
                permit = tx.reserve() => {
                    match permit { Ok(permit) => { permit.send(event); emitted += 1; break; }, Err(_) => return Ok((false, emitted)) }
                }
                _ = state.changed() => if !ready(&mut state, tx, control).await { return Ok((false, emitted)); },
            }
        }
    }
    Ok((true, emitted))
}

pub async fn scan(
    path: PathBuf,
    format: MarauderFormat,
    tx: mpsc::Sender<ScanEvent>,
    control: ScanControl,
) -> io::Result<()> {
    let result = consume(path, format, &tx, &control).await;
    let (state, count) = match &result {
        Ok((true, count)) => ("finished", *count),
        Ok((false, count)) => ("stopped", *count),
        Err(_) => ("failed", 0),
    };
    control.set_health(
        Scanner::Wifi,
        if result.is_err() {
            Phase::Unavailable
        } else {
            Phase::Stopped
        },
        "marauder_capture_replay",
    );
    let status = ScanEvent::WifiReplayStatus(
        serde_json::json!({"type":"marauder_replay_status", "schema_version":1, "replay":true, "state":state, "emitted_events":count, "radio_loss":"unknown"}),
    );
    let mut shutdown = control.subscribe();
    if shutdown.borrow().shutdown {
        let _ = tx.try_send(status);
    } else {
        tokio::select! { _=tx.send(status)=>{}, _=shutdown.changed()=>{} }
    }
    result.map(|_| ())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    pub(crate) fn fixture_bytes() -> Vec<u8> {
        let mut data = vec![0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0];
        for n in [0u32, 0, 1514, 105] {
            data.extend(n.to_le_bytes());
        }
        let mut beacon = vec![0u8; 36];
        beacon[0] = 0x80;
        beacon[10..16].copy_from_slice(&[0x02, 1, 2, 3, 4, 5]);
        beacon[16..22].copy_from_slice(&[0x02, 1, 2, 3, 4, 5]);
        beacon.extend([0, 7]);
        beacon.extend(b"fixture");
        beacon.extend([3, 1, 6]);
        for (ticks, frame) in [(u32::MAX, beacon), (7, vec![0x08, 0, 0, 0])] {
            for n in [
                ticks / 1_000_000,
                ticks % 1_000_000,
                frame.len() as u32,
                frame.len() as u32,
            ] {
                data.extend(n.to_le_bytes());
            }
            data.extend(frame);
        }
        data
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new(data: &[u8]) -> Self {
            let path = std::env::temp_dir().join(format!(
                "airhound-marauder-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::write(&path, data).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[tokio::test]
    async fn marauder_validates_complete_input_before_any_packet_and_preserves_wrap() {
        for serial in [false, true] {
            let data = fixture_bytes();
            let bytes = if serial {
                [b"boot[BUF/BEGIN]".as_slice(), &data, b"[BUF/CLOSE]stopped"].concat()
            } else {
                data
            };
            let f = Fixture::new(&bytes);
            let (tx, mut rx) = mpsc::channel(8);
            scan(
                f.0.clone(),
                if serial {
                    MarauderFormat::Serial
                } else {
                    MarauderFormat::Pcap
                },
                tx,
                ScanControl::new(true),
            )
            .await
            .unwrap();
            let mut ticks = vec![];
            while let Some(event) = rx.recv().await {
                if let ScanEvent::MarauderReplay {
                    timestamp_micros32, ..
                } = event
                {
                    ticks.push(timestamp_micros32)
                }
            }
            assert_eq!(ticks, [u32::MAX, 7]);
        }
        let mut bytes = fixture_bytes();
        bytes.pop();
        let f = Fixture::new(&bytes);
        let (tx, mut rx) = mpsc::channel(8);
        assert!(scan(
            f.0.clone(),
            MarauderFormat::Pcap,
            tx,
            ScanControl::new(true)
        )
        .await
        .is_err());
        while let Some(event) = rx.recv().await {
            assert!(!matches!(event, ScanEvent::MarauderReplay { .. }));
        }
    }
    async fn pending<F: std::future::Future>(mut future: std::pin::Pin<&mut F>) {
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(
                future.as_mut().poll(cx).is_pending()
            ))
            .await
        );
    }
    async fn event_while_running<F: std::future::Future>(
        mut source: std::pin::Pin<&mut F>,
        rx: &mut mpsc::Receiver<ScanEvent>,
    ) -> ScanEvent {
        tokio::select! {
            event = rx.recv() => event.expect("source ended before event"),
            _ = &mut source => panic!("source completed before expected event"),
        }
    }
    #[tokio::test]
    async fn marauder_paused_queue_stops_without_dropping_into_live_semantics() {
        crate::test_support::manual_time(async {
            let f = Fixture::new(&fixture_bytes());
            let (tx, mut rx) = mpsc::channel(1);
            let control = ScanControl::new(false);
            let mut source = Box::pin(scan(f.0.clone(), MarauderFormat::Pcap, tx, control.clone()));
            pending(source.as_mut()).await; // actual pause wait precedes filesystem access
            assert!(rx.try_recv().is_err());
            control.store(true, Ordering::Relaxed);
            assert!(matches!(
                event_while_running(source.as_mut(), &mut rx).await,
                ScanEvent::MarauderReplay {
                    timestamp_micros32: u32::MAX,
                    ..
                }
            ));
            control.shutdown();
            let (result, ()) = tokio::join!(source, async { while rx.recv().await.is_some() {} });
            result.unwrap();
        })
        .await;
    }
    #[tokio::test]
    async fn marauder_backpressure_pause_resume_restores_running_health() {
        crate::test_support::manual_time(async {
            let original = fixture_bytes();
            let mut bytes = original[..24].to_vec();
            for _ in 0..20 {
                bytes.extend(&original[24..]);
            }
            let f = Fixture::new(&bytes);
            let (tx, mut rx) = mpsc::channel(1);
            let control = ScanControl::new(true);
            let mut source = Box::pin(scan(f.0.clone(), MarauderFormat::Pcap, tx, control.clone()));
            assert!(matches!(
                event_while_running(source.as_mut(), &mut rx).await,
                ScanEvent::MarauderReplay { .. }
            ));
            // File input is now decoded. Drive the next send until the bounded
            // output queue is full, then observe Pause at that exact boundary.
            pending(source.as_mut()).await;
            assert_eq!(rx.len(), 1);
            control.store(false, Ordering::Relaxed);
            pending(source.as_mut()).await;
            assert_eq!(control.health().wifi.state, Phase::Paused);
            assert!(matches!(
                rx.try_recv(),
                Ok(ScanEvent::MarauderReplay { .. })
            ));
            pending(source.as_mut()).await; // pause wait cannot publish another record
            assert!(rx.try_recv().is_err());
            control.store(true, Ordering::Relaxed);
            assert!(matches!(
                event_while_running(source.as_mut(), &mut rx).await,
                ScanEvent::MarauderReplay { .. }
            ));
            assert_eq!(control.health().wifi.state, Phase::Running);
            control.shutdown();
            let (result, ()) = tokio::join!(source, async { while rx.recv().await.is_some() {} });
            result.unwrap();
        })
        .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn marauder_rejects_fifo_and_device_nodes_without_waiting() {
        crate::test_support::manual_time(async {
            let path =
                std::env::temp_dir().join(format!("airhound-marauder-fifo-{}", std::process::id()));
            let name = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            let _f = Fixture(path.clone());
            let saved = Fixture::new(&fixture_bytes());
            let link = saved.0.with_extension("link");
            std::os::unix::fs::symlink(&saved.0, &link).unwrap();
            let _link = Fixture(link.clone());
            for path in [path, link, PathBuf::from("/dev/null")] {
                let (tx, _rx) = mpsc::channel(8);
                assert!(scan(path, MarauderFormat::Pcap, tx, ScanControl::new(true))
                    .await
                    .is_err());
            }
        })
        .await;
    }
}
