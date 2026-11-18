//! Receive-only capture on an externally configured monitor-mode interface.
use airhound::scanner::ScanEvent;
use tokio::sync::mpsc;

pub async fn scan_loop(
    interface: &str,
    tx: mpsc::Sender<ScanEvent>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut capture = pcap::Capture::from_device(interface)?
        .snaplen(65535)
        .timeout(100)
        .open()?
        .setnonblock()?;
    if capture.get_datalink() != pcap::Linktype::IEEE802_11_RADIOTAP {
        return Err("WiFi capture requires a monitor-mode interface with radiotap headers".into());
    }
    log::info!("WiFi capture on {interface}; channel is controlled externally");
    loop {
        if tx.is_closed() {
            return Ok(());
        }
        match capture.next_packet() {
            Ok(packet) => {
                if packet.header.caplen != packet.header.len {
                    continue;
                }
                for event in crate::radiotap::events(packet.data) {
                    let _ = tx.try_send(event);
                }
                // Bound continuous traffic work so cancellation can always run.
                tokio::task::yield_now().await;
            }
            Err(pcap::Error::TimeoutExpired) => {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await
            }
            Err(e) => return Err(e.into()),
        }
    }
}
