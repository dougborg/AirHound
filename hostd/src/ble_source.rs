//! Explicit external BLE source. Child process is separate from the MIT matcher.
use crate::{
    ble_packet::Reader,
    control::{wait_running, ScanControl},
    health::{Phase, Scanner},
    observation::ScanEvent,
};
use std::{io, path::PathBuf, process::Stdio, time::Duration};
use tokio::{io::AsyncRead, process::Command, sync::mpsc};

#[derive(Clone, Debug)]
pub enum Source {
    Pcap(PathBuf),
    Sniffle {
        port: String,
        python: PathBuf,
        helper: PathBuf,
        module_path: Option<PathBuf>,
        channel: u8,
        coded: bool,
    },
}
#[derive(Default)]
struct Stats {
    packets: u64,
    host_drops: u64,
    generation: u64,
    reconnects: u64,
}
fn status(stats: &Stats, state: &str) -> ScanEvent {
    ScanEvent::BleSourceStatus(
        serde_json::json!({"type":"ble_source_status","schema_version":1,"state":state,"packets":stats.packets,"host_dropped_packets":stats.host_drops,"generation":stats.generation,"reconnect_attempts":stats.reconnects,"radio_loss":"unknown"}),
    )
}

async fn consume<R: AsyncRead + Unpin>(
    input: R,
    source_id: &str,
    replay: bool,
    tx: &mpsc::Sender<ScanEvent>,
    control: &ScanControl,
    stats: &mut Stats,
) -> io::Result<bool> {
    let mut state = control.subscribe();
    let header = tokio::time::timeout(Duration::from_secs(10), Reader::new(input));
    tokio::pin!(header);
    let mut reader = loop {
        tokio::select! {
            r=&mut header=>break r.map_err(|_|io::Error::new(io::ErrorKind::TimedOut,"BLE source header timeout"))??,
            _=state.changed()=>{if !replay || !wait_running(&mut state,tx).await{return Ok(false)}},
            _=tx.closed()=>return Ok(false),
        }
    };
    control.set_health(
        Scanner::Ble,
        Phase::Running,
        if replay {
            "pcap_replay"
        } else {
            "sniffle_passive"
        },
    );
    control.set_receiver_availability("available");
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    let mut sequence = 0u64;
    loop {
        let next = reader.next();
        tokio::pin!(next);
        let record = loop {
            tokio::select! {
                r=&mut next=>break r?,
                _=state.changed()=>{
                    if !replay || !wait_running(&mut state,tx).await { return Ok(false); }
                },
                _=tx.closed()=>return Ok(false),
                _=tick.tick()=>{let _=tx.try_send(status(stats,"running"));},
            }
        };
        let Some(mut packet) = record else {
            return Ok(true);
        };
        sequence += 1;
        stats.packets += 1;
        packet.source_id = source_id.into();
        packet.generation = stats.generation;
        packet.sequence = sequence;
        packet.replay = replay;
        packet.ingestion_generation = state.borrow().generation;
        if let Some(snapshot) = packet.advertisement.as_mut() {
            snapshot.receiver_id = Some(source_id.into());
        }
        let event = ScanEvent::BlePacket(Box::new(packet));
        if replay {
            let send = tx.send(event);
            tokio::pin!(send);
            loop {
                tokio::select! {r=&mut send=>{if r.is_err(){return Ok(false)}break;},_=state.changed()=>{if !wait_running(&mut state,tx).await{return Ok(false)}}}
            }
        } else if let Err(e) = tx.try_send(event) {
            if matches!(e, mpsc::error::TrySendError::Closed(_)) {
                return Ok(false);
            }
            stats.host_drops += 1;
        }
    }
}

// Drain bounded lines concurrently so diagnostics cannot block the PCAP stream.
// Only the strict identity message changes metadata; all other stderr is diagnostic.
async fn read_identity(mut input: impl AsyncRead + Unpin, control: ScanControl) {
    use tokio::io::AsyncReadExt;
    let mut chunk = [0u8; 1024];
    let mut line = Vec::new();
    let mut oversized = false;
    while let Ok(count) = input.read(&mut chunk).await {
        if count == 0 {
            break;
        }
        for byte in &chunk[..count] {
            if *byte == b'\n' {
                if !oversized {
                    if let Ok(firmware) =
                        airhound_hostd::discovery::identify::Firmware::parse(&line)
                    {
                        control.set_receiver_firmware(Some(firmware));
                    } else if !line.is_empty() {
                        log::debug!("Sniffle helper: {:?}", String::from_utf8_lossy(&line));
                    }
                }
                line.clear();
                oversized = false;
            } else if line.len() < 4096 && !oversized {
                line.push(*byte);
            } else {
                oversized = true;
                line.clear();
            }
        }
    }
}

pub async fn scan(
    source: Source,
    tx: mpsc::Sender<ScanEvent>,
    control: ScanControl,
) -> io::Result<()> {
    let mut state = control.subscribe();
    let mut stats = Stats::default();
    let (backend, id, info, configuration) = match &source {
        Source::Pcap(_) => (
            "ble_pcap",
            "ble-pcap".into(),
            "Classic DLT256 capture replay".into(),
            "recorded_capture",
        ),
        Source::Sniffle {
            port,
            channel,
            coded,
            ..
        } => (
            "sniffle",
            format!("sniffle:{port}"),
            format!(
                "Sniffle source, channel {channel}, coded={coded}; firmware not yet identified"
            ),
            "passive_fixed_channel",
        ),
    };
    control.set_receiver(crate::receiver::Receiver {
        backend,
        adapter_id: id,
        adapter_info: info,
        firmware: None,
        controller_address: None,
        selection: "explicit",
        availability: "initializing",
        scan_configuration: configuration,
        extended_advertising: Default::default(),
        coded_rx: Default::default(),
        controller_phy: None,
        capability_query: "raw_packet_metadata_only",
    });
    match source {
        Source::Pcap(path) => {
            // Replay is finite and never silently restarted from the beginning on pause.
            if !tokio::fs::metadata(&path).await?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "BLE replay requires a regular saved PCAP file",
                ));
            }
            let file = tokio::fs::File::open(path).await?;
            if !wait_running(&mut state, &tx).await {
                return Ok(());
            }
            let result = consume(file, "ble-pcap", true, &tx, &control, &mut stats).await;
            let completed = matches!(result, Ok(true));
            control.set_health(
                Scanner::Ble,
                if result.is_err() {
                    Phase::Unavailable
                } else {
                    Phase::Stopped
                },
                if result.is_err() {
                    "pcap_replay_failed"
                } else if completed {
                    "pcap_replay_finished"
                } else {
                    "pcap_replay_stopped"
                },
            );
            control.set_receiver_availability(if result.is_err() {
                "unavailable"
            } else {
                "stopped"
            });
            let _ = tx
                .send(status(
                    &stats,
                    if result.is_err() {
                        "failed"
                    } else if completed {
                        "finished"
                    } else {
                        "stopped"
                    },
                ))
                .await;
            result.map(|_| ())
        }
        Source::Sniffle {
            port,
            python,
            helper,
            module_path,
            channel,
            coded,
        } => {
            let mut consecutive = 0;
            loop {
                if !wait_running(&mut state, &tx).await {
                    return Ok(());
                }
                stats.generation += 1;
                control.set_receiver_firmware(None);
                control.set_receiver_availability("initializing");
                control.set_health(Scanner::Ble, Phase::Initializing, "sniffle_starting");
                let mut command = Command::new(&python);
                command
                    .arg(&helper)
                    .arg("--port")
                    .arg(&port)
                    .arg("--channel")
                    .arg(channel.to_string())
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true);
                if let Some(path) = &module_path {
                    command.arg("--module-path").arg(path);
                }
                if coded {
                    command.arg("--coded");
                }
                let mut child = command.spawn()?;
                let identity_control = control.clone();
                let stderr = child.stderr.take().expect("piped stderr");
                let identity =
                    tokio::spawn(async move { read_identity(stderr, identity_control).await });
                let before = stats.packets;
                let result = consume(
                    child.stdout.take().expect("piped stdout"),
                    &format!("sniffle:{port}"),
                    false,
                    &tx,
                    &control,
                    &mut stats,
                )
                .await;
                // Reap on EOF, pause, shutdown, malformed input, and output closure.
                let _ = child.start_kill();
                let _ = child.wait().await;
                identity.abort();
                let _ = identity.await;
                if state.borrow().shutdown || tx.is_closed() {
                    control.set_health(Scanner::Ble, Phase::Stopped, "sniffle_source_stopped");
                    control.set_receiver_firmware(None);
                    control.set_receiver_availability("stopped");
                    let _ = tx.send(status(&stats, "stopped")).await;
                    return Ok(());
                }
                if !state.borrow().enabled {
                    control.set_health(Scanner::Ble, Phase::Paused, "sniffle_ingestion_paused");
                    control.set_receiver_firmware(None);
                    control.set_receiver_availability("paused");
                    let _ = tx.send(status(&stats, "paused")).await;
                    continue;
                }
                if stats.packets > before {
                    consecutive = 0;
                }
                consecutive += 1;
                stats.reconnects += 1;
                control.set_receiver_firmware(None);
                control.set_receiver_availability("unavailable");
                control.set_health(Scanner::Ble, Phase::Unavailable, "sniffle_disconnected");
                let _ = tx.send(status(&stats, "reconnecting")).await;
                if consecutive >= 3 {
                    return Err(result.err().unwrap_or_else(|| {
                        io::Error::new(io::ErrorKind::BrokenPipe, "Sniffle exited repeatedly")
                    }));
                }
                tokio::select! {_=tokio::time::sleep(Duration::from_secs(2))=>{},_=state.changed()=>{},_=tx.closed()=>return Ok(())}
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::ble_packet::tests::{frame, pcap};
    use tokio::io::AsyncWriteExt;
    #[tokio::test]
    async fn firmware_identity_uses_bounded_valid_messages_and_clears_on_generation_change() {
        let c = ScanControl::new(true);
        c.set_receiver(crate::receiver::Receiver {
            backend: "sniffle",
            adapter_id: "sniffle:/dev/test".into(),
            adapter_info: "unidentified".into(),
            firmware: None,
            controller_address: None,
            selection: "explicit",
            availability: "initializing",
            scan_configuration: "passive_fixed_channel",
            extended_advertising: Default::default(),
            coded_rx: Default::default(),
            controller_phy: None,
            capability_query: "raw_packet_metadata_only",
        });
        assert!(c.health_message()["receiver"].get("firmware").is_none());
        let mut bytes = vec![b'x'; 5000];
        bytes.push(b'\n');
        bytes.extend_from_slice(b"not identity\n{}\n");
        bytes.extend_from_slice(br#"{"type":"sniffle_identity","schema_version":1,"major":1,"minor":11,"patch":0,"api_level":0}"#);
        bytes.push(b'\n');
        read_identity(bytes.as_slice(), c.clone()).await;
        let receiver = &c.health_message()["receiver"];
        assert_eq!(receiver["firmware"]["minor"], 11);
        assert!(receiver["adapter_info"]
            .as_str()
            .unwrap()
            .contains("reported by firmware"));
        c.set_receiver_firmware(None);
        assert!(c.health_message()["receiver"].get("firmware").is_none());
        assert!(!c.health_message()["receiver"]["adapter_info"]
            .as_str()
            .unwrap()
            .contains("1.11.0"));
    }
    #[tokio::test]
    async fn stream_pressure_is_counted_but_replay_is_lossless() {
        let bytes = pcap(&vec![frame(0, &[]); 5]);
        let (tx, mut rx) = mpsc::channel(2);
        let c = ScanControl::new(true);
        let mut stats = Stats::default();
        assert!(
            consume(bytes.as_slice(), "test", false, &tx, &c, &mut stats)
                .await
                .unwrap()
        );
        assert_eq!(stats.packets, 5);
        assert!(stats.host_drops >= 3);
        while rx.try_recv().is_ok() {}
        let producer = async {
            let mut stats = Stats::default();
            assert!(consume(bytes.as_slice(), "test", true, &tx, &c, &mut stats)
                .await
                .unwrap());
            stats
        };
        let consumer = async {
            let mut packets = 0;
            while packets < 5 {
                if let Some(ScanEvent::BlePacket(p)) = rx.recv().await {
                    packets += 1;
                    assert!(p.replay);
                    assert_eq!(p.sequence, packets);
                    assert_eq!(p.source_id, "test");
                }
            }
        };
        let (stats, ()) = tokio::join!(producer, consumer);
        assert_eq!(stats.host_drops, 0);
    }
    #[tokio::test(start_paused = true)]
    async fn partial_record_survives_status_tick() {
        let (mut writer, reader) = tokio::io::duplex(256);
        let bytes = pcap(&[frame(0, &[])]);
        let (tx, mut rx) = mpsc::channel(10);
        let c = ScanControl::new(true);
        let mut stats = Stats::default();
        let producer = async {
            consume(reader, "test", false, &tx, &c, &mut stats)
                .await
                .unwrap()
        };
        let feed = async {
            writer.write_all(&bytes[..30]).await.unwrap();
            tokio::time::sleep(Duration::from_secs(6)).await;
            writer.write_all(&bytes[30..]).await.unwrap();
            writer.shutdown().await.unwrap();
        };
        let (eof, ()) = tokio::join!(producer, feed);
        assert!(eof);
        assert_eq!(stats.packets, 1);
        let mut count = 0;
        while let Ok(e) = rx.try_recv() {
            if matches!(e, ScanEvent::BlePacket(_)) {
                count += 1
            }
        }
        assert_eq!(count, 1);
    }
    #[tokio::test(start_paused = true)]
    async fn header_timeout_and_closed_output_are_bounded() {
        let (_w, r) = tokio::io::duplex(128);
        let (tx, _rx) = mpsc::channel(1);
        let c = ScanControl::new(true);
        let mut stats = Stats::default();
        assert_eq!(
            consume(r, "test", false, &tx, &c, &mut stats)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        let (_w, r) = tokio::io::duplex(128);
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        assert!(!consume(r, "test", false, &tx, &c, &mut stats)
            .await
            .unwrap());
    }
    #[tokio::test]
    async fn pause_and_resume_replay_without_restarting_or_losing_partial_record() {
        let (mut w, r) = tokio::io::duplex(256);
        let bytes = pcap(&[frame(0, &[])]);
        let (tx, mut rx) = mpsc::channel(8);
        let c = ScanControl::new(true);
        let mut stats = Stats::default();
        let feed = async {
            w.write_all(&bytes[..30]).await.unwrap();
            tokio::time::sleep(Duration::from_millis(5)).await;
            c.store(false, std::sync::atomic::Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(5)).await;
            c.store(true, std::sync::atomic::Ordering::Relaxed);
            w.write_all(&bytes[30..]).await.unwrap();
            w.shutdown().await.unwrap();
        };
        let (result, ()) = tokio::join!(consume(r, "test", true, &tx, &c, &mut stats), feed);
        assert!(result.unwrap());
        assert_eq!(stats.packets, 1);
        assert!(
            std::iter::from_fn(|| rx.try_recv().ok()).any(|e| matches!(e, ScanEvent::BlePacket(_)))
        );
    }
    #[tokio::test]
    async fn live_pause_and_shutdown_cancel_pending_reads() {
        for shutdown in [false, true] {
            let (mut w, r) = tokio::io::duplex(256);
            let bytes = pcap(&[]);
            let (tx, _rx) = mpsc::channel(8);
            let c = ScanControl::new(true);
            let mut stats = Stats::default();
            let feed = async {
                w.write_all(&bytes).await.unwrap();
                tokio::time::sleep(Duration::from_millis(5)).await;
                if shutdown {
                    c.shutdown()
                } else {
                    c.store(false, std::sync::atomic::Ordering::Relaxed)
                }
            };
            let (result, ()) = tokio::join!(consume(r, "test", false, &tx, &c, &mut stats), feed);
            assert!(!result.unwrap());
        }
    }
    #[tokio::test]
    async fn explicit_file_failures_and_replay_completion() {
        let (tx, mut rx) = mpsc::channel(8);
        let c = ScanControl::new(true);
        assert!(scan(
            Source::Pcap("/no-such-airhound-pcap".into()),
            tx.clone(),
            c.clone()
        )
        .await
        .is_err());
        assert_eq!(
            scan(Source::Pcap(std::env::temp_dir()), tx.clone(), c.clone())
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        let path =
            std::env::temp_dir().join(format!("airhound-ble-replay-{}.pcap", std::process::id()));
        std::fs::write(&path, pcap(&[frame(0, &[])])).unwrap();
        scan(Source::Pcap(path.clone()), tx.clone(), c.clone())
            .await
            .unwrap();
        assert_eq!(c.health().ble.state, Phase::Stopped);
        assert!(
            std::iter::from_fn(|| rx.try_recv().ok()).any(|e| matches!(e, ScanEvent::BlePacket(_)))
        );
        std::fs::write(&path, b"bad pcap").unwrap();
        assert!(scan(Source::Pcap(path.clone()), tx, c).await.is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn exited_child_retries_then_fails_without_native_fallback() {
        let (tx, mut rx) = mpsc::channel(32);
        let c = ScanControl::new(true);
        let source = Source::Sniffle {
            port: "test".into(),
            python: "/usr/bin/false".into(),
            helper: "unused".into(),
            module_path: Some("unused".into()),
            channel: 39,
            coded: true,
        };
        assert!(scan(source, tx, c.clone()).await.is_err());
        assert!(c.health_message()["receiver"].get("firmware").is_none());
        assert!(!c.health_message()["receiver"]["adapter_info"]
            .as_str()
            .unwrap()
            .contains("1.11.0"));
        let messages: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(messages.len(), 3);
    }
}
