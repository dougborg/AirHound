//! Receive-only capture on an externally configured monitor-mode interface.
use crate::ScanEvent;
use tokio::sync::mpsc;

pub async fn scan_loop(
    interface: &str,
    tx: mpsc::Sender<ScanEvent>,
    control: crate::control::ScanControl,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let capture = pcap::Capture::from_device(interface)?
        .snaplen(65535)
        .timeout(100)
        .open()?
        .setnonblock()?;
    log::info!("WiFi capture on {interface}; channel is controlled externally");
    capture_loop(capture, tx, control).await
}

async fn capture_loop<T: pcap::Activated>(
    mut capture: pcap::Capture<T>,
    tx: mpsc::Sender<ScanEvent>,
    control: crate::control::ScanControl,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if capture.get_datalink() != pcap::Linktype::IEEE802_11_RADIOTAP {
        return Err("WiFi capture requires a monitor-mode interface with radiotap headers".into());
    }
    let mut state = control.subscribe();
    loop {
        if !crate::control::wait_running(&mut state, &tx).await {
            return Ok(());
        }
        control.set_health(
            crate::health::Scanner::Wifi,
            crate::health::Phase::Running,
            "capture_active",
        );
        // Even a stream of truncated packets must allow cancellation to run.
        tokio::task::yield_now().await;
        match capture.next_packet() {
            Ok(packet) => {
                if packet.header.caplen != packet.header.len {
                    continue;
                }
                for event in crate::radiotap::events(packet.data) {
                    let _ = tx.try_send(event);
                }
            }
            Err(pcap::Error::TimeoutExpired) => {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await
            }
            Err(pcap::Error::NoMorePackets) => return Ok(()),
            Err(e) => return Err(e.into()),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    fn capture(link: u32, packets: &[(&[u8], u32)]) -> pcap::Capture<pcap::Offline> {
        let path = std::env::temp_dir().join(format!(
            "airhound-pcap-{}-{}.pcap",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(&[0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0])
            .unwrap();
        for value in [0u32, 0, 65535, link] {
            file.write_all(&value.to_le_bytes()).unwrap();
        }
        for (data, original) in packets {
            for value in [0u32, 0, data.len() as u32, *original] {
                file.write_all(&value.to_le_bytes()).unwrap();
            }
            file.write_all(data).unwrap();
        }
        drop(file);
        let capture = pcap::Capture::from_file(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        capture
    }

    fn beacon() -> Vec<u8> {
        // Radiotap: channel 1 and -47dBm; beacon with Flock SSID.
        let mut packet = vec![
            0,
            0,
            13,
            0,
            0x28,
            0,
            0,
            0,
            0x6c,
            0x09,
            0x80,
            0,
            (-47i8) as u8,
        ];
        let mut frame = vec![0; 36];
        frame[0] = 0x80;
        frame[10..16].copy_from_slice(&[0x12; 6]);
        frame.extend_from_slice(&[0, 12]);
        frame.extend_from_slice(b"Flock-123456");
        packet.extend(frame);
        packet
    }

    #[tokio::test]
    async fn offline_capture_rejects_truncation_and_malformed_frames_and_handles_eof() {
        let packet = beacon();
        let capture = capture(
            127,
            &[
                (&packet, packet.len() as u32 + 1),
                (&[0, 1], 2),
                (&packet, packet.len() as u32),
            ],
        );
        let (tx, mut rx) = mpsc::channel(8);
        capture_loop(capture, tx, crate::control::ScanControl::new(true))
            .await
            .unwrap();
        let ScanEvent::WiFi(event) = rx.recv().await.unwrap() else {
            panic!("expected WiFi")
        };
        assert_eq!(event.ssid.as_str(), "Flock-123456");
        assert_eq!(event.rssi, -47);
        assert_eq!(event.channel, 1);
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn capture_rejects_wrong_linktype_and_handles_closed_or_saturated_output() {
        let (tx, _rx) = mpsc::channel(1);
        assert!(
            capture_loop(capture(1, &[]), tx, crate::control::ScanControl::new(true))
                .await
                .unwrap_err()
                .to_string()
                .contains("radiotap")
        );
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        capture_loop(
            capture(127, &[]),
            tx,
            crate::control::ScanControl::new(true),
        )
        .await
        .unwrap();
        let packet = beacon();
        let (tx, mut rx) = mpsc::channel(1);
        capture_loop(
            capture(
                127,
                &[
                    (&packet, packet.len() as u32),
                    (&packet, packet.len() as u32),
                ],
            ),
            tx,
            crate::control::ScanControl::new(true),
        )
        .await
        .unwrap();
        assert!(rx.recv().await.is_some());
        assert!(rx.recv().await.is_none());
    }
}
