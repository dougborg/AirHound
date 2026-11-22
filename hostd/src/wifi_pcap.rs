//! Offline, bounded classic PCAP replay. No radio, helper or device node access.
use crate::{
    control::{wait_running, ScanControl},
    health::{Phase, Scanner},
    observation::ScanEvent,
};
use std::{io, path::PathBuf};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::mpsc,
};

const MAX_CAPTURE: u32 = 65_536;
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn number(bytes: &[u8], little: bool) -> u32 {
    let bytes = bytes.try_into().expect("four bytes");
    if little {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Metadata {
    pub captured_at_us: u64,
    pub record_sequence: u64,
    pub captured_len: u32,
    pub original_len: u32,
    pub timestamp_resolution: &'static str,
}
impl Metadata {
    pub fn csv_time(&self) -> String {
        chrono::DateTime::from_timestamp_micros(self.captured_at_us as i64)
            .expect("classic PCAP u32 seconds fits chrono")
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    }
}
struct Record {
    metadata: Metadata,
    bytes: Vec<u8>,
}
struct Reader<R> {
    input: R,
    little: bool,
    nanos: bool,
    snaplen: u32,
    sequence: u64,
}
impl<R: AsyncRead + Unpin> Reader<R> {
    async fn new(mut input: R) -> io::Result<Self> {
        let mut header = [0u8; 24];
        input.read_exact(&mut header).await?;
        let (little, nanos) = match &header[..4] {
            [0xd4, 0xc3, 0xb2, 0xa1] => (true, false),
            [0xa1, 0xb2, 0xc3, 0xd4] => (false, false),
            [0x4d, 0x3c, 0xb2, 0xa1] => (true, true),
            [0xa1, 0xb2, 0x3c, 0x4d] => (false, true),
            _ => return Err(invalid("WiFi replay requires classic PCAP, not PCAPNG")),
        };
        let version = if little { [2, 0, 4, 0] } else { [0, 2, 0, 4] };
        if header[4..8] != version || number(&header[20..24], little) != 127 {
            return Err(invalid(
                "WiFi replay requires PCAP 2.4 radiotap link type 127",
            ));
        }
        let snaplen = number(&header[16..20], little);
        if snaplen == 0 || snaplen > 16 * 1024 * 1024 {
            return Err(invalid("invalid PCAP snaplen"));
        }
        Ok(Self {
            input,
            little,
            nanos,
            snaplen,
            sequence: 0,
        })
    }
    async fn next(&mut self) -> io::Result<Option<Record>> {
        let mut header = [0u8; 16];
        if self.input.read(&mut header[..1]).await? == 0 {
            return Ok(None);
        }
        self.input.read_exact(&mut header[1..]).await?;
        let seconds = number(&header[..4], self.little);
        let subsecond = number(&header[4..8], self.little);
        let captured_len = number(&header[8..12], self.little);
        let original_len = number(&header[12..], self.little);
        if captured_len == 0
            || captured_len > MAX_CAPTURE
            || captured_len > self.snaplen
            || captured_len > original_len
            || subsecond >= if self.nanos { 1_000_000_000 } else { 1_000_000 }
        {
            return Err(invalid("invalid or oversized WiFi PCAP record"));
        }
        let mut bytes = vec![0u8; captured_len as usize];
        self.input.read_exact(&mut bytes).await?;
        self.sequence += 1;
        Ok(Some(Record {
            metadata: Metadata {
                captured_at_us: u64::from(seconds) * 1_000_000
                    + u64::from(if self.nanos {
                        subsecond / 1000
                    } else {
                        subsecond
                    }),
                record_sequence: self.sequence,
                captured_len,
                original_len,
                timestamp_resolution: if self.nanos {
                    "nanoseconds_truncated_to_microseconds"
                } else {
                    "microseconds"
                },
            },
            bytes,
        }))
    }
}
#[derive(Default)]
struct Stats {
    records: u64,
    rejected: u64,
    events: u64,
}
async fn consume<R: AsyncRead + Unpin>(
    input: R,
    tx: &mpsc::Sender<ScanEvent>,
    control: &ScanControl,
    stats: &mut Stats,
) -> io::Result<bool> {
    let mut state = control.subscribe();
    if !wait_running(&mut state, tx).await {
        return Ok(false);
    }
    let header = Reader::new(input);
    tokio::pin!(header);
    let mut reader = loop {
        tokio::select! {
            result=&mut header => break result?,
            _=state.changed()=> if !wait_running(&mut state,tx).await {return Ok(false)},
            _=tx.closed()=>return Ok(false),
        }
    };
    control.set_health(Scanner::Wifi, Phase::Running, "wifi_pcap_replay");
    loop {
        if !wait_running(&mut state, tx).await {
            return Ok(false);
        }
        let next = reader.next();
        tokio::pin!(next);
        let record = loop {
            tokio::select! {
                result=&mut next=>break result?,
                _=state.changed()=> if !wait_running(&mut state,tx).await {return Ok(false)},
                _=tx.closed()=>return Ok(false),
            }
        };
        let Some(record) = record else {
            return Ok(true);
        };
        stats.records += 1;
        if record.metadata.captured_len != record.metadata.original_len {
            stats.rejected += 1;
            continue;
        }
        let mut decoded = false;
        for event in crate::radiotap::events(&record.bytes) {
            decoded = true;
            let send = tx.send(ScanEvent::Replay {
                event: Box::new(event),
                metadata: record.metadata.clone(),
            });
            tokio::pin!(send);
            loop {
                tokio::select! {
                    result=&mut send=>{if result.is_err(){return Ok(false)} break},
                    _=state.changed()=>if !wait_running(&mut state,tx).await{return Ok(false)},
                }
            }
            stats.events += 1;
        }
        if !decoded {
            stats.rejected += 1;
        }
    }
}

pub async fn scan(
    path: PathBuf,
    tx: mpsc::Sender<ScanEvent>,
    control: ScanControl,
) -> io::Result<()> {
    let mut stats = Stats::default();
    let result = async {
        if !tokio::fs::metadata(&path).await?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "WiFi replay requires a regular saved PCAP file",
            ));
        }
        consume(
            tokio::fs::File::open(path).await?,
            &tx,
            &control,
            &mut stats,
        )
        .await
    }
    .await;
    let (state, reason) = match &result {
        Ok(true) => ("finished", "wifi_pcap_replay_finished"),
        Ok(false) => ("stopped", "wifi_pcap_replay_stopped"),
        Err(_) => ("failed", "wifi_pcap_replay_failed"),
    };
    control.set_health(
        Scanner::Wifi,
        if result.is_err() {
            Phase::Unavailable
        } else {
            Phase::Stopped
        },
        reason,
    );
    let status = ScanEvent::WifiReplayStatus(serde_json::json!({
        "type":"wifi_replay_status","schema_version":1,"replay":true,"state":state,
        "records":stats.records,"rejected_records":stats.rejected,"emitted_events":stats.events,
        "radio_loss":"unknown","host_dropped_observations":0
    }));
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
mod tests {
    use super::*;
    pub fn pcap(little: bool, nanos: bool, packets: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = match (little, nanos) {
            (true, false) => vec![0xd4, 0xc3, 0xb2, 0xa1],
            (false, false) => vec![0xa1, 0xb2, 0xc3, 0xd4],
            (true, true) => vec![0x4d, 0x3c, 0xb2, 0xa1],
            (false, true) => vec![0xa1, 0xb2, 0x3c, 0x4d],
        };
        bytes.extend(if little { [2, 0, 4, 0] } else { [0, 2, 0, 4] });
        let append = |bytes: &mut Vec<u8>, n: u32| {
            bytes.extend(if little {
                n.to_le_bytes()
            } else {
                n.to_be_bytes()
            })
        };
        for n in [0, 0, 65536, 127] {
            append(&mut bytes, n);
        }
        for packet in packets {
            for n in [
                1_700_000_000,
                if nanos { 123_456_789 } else { 123_456 },
                packet.len() as u32,
                packet.len() as u32,
            ] {
                append(&mut bytes, n);
            }
            bytes.extend(packet);
        }
        bytes
    }
    pub fn frame() -> Vec<u8> {
        let mut bytes = vec![0, 0, 13, 0, 0x28, 0, 0, 0];
        bytes.extend(5955u16.to_le_bytes());
        bytes.extend([0x40, 1, (-40i8) as u8]);
        let mut frame = [0u8; 24];
        frame[0] = 8;
        frame[10..16].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        bytes.extend(frame);
        bytes
    }
    #[tokio::test]
    async fn endian_timestamp_and_record_bounds() {
        for little in [false, true] {
            for nanos in [false, true] {
                let bytes = pcap(little, nanos, &[frame()]);
                let mut reader = Reader::new(bytes.as_slice()).await.unwrap();
                let record = reader.next().await.unwrap().unwrap();
                assert_eq!(record.metadata.captured_at_us, 1_700_000_000_123_456);
                assert_eq!(record.metadata.record_sequence, 1);
                assert_eq!(record.metadata.csv_time(), "2023-11-14 22:13:20");
                assert!(reader.next().await.unwrap().is_none());
            }
        }
        for (offset, value) in [(20, 256), (16, 0), (4, 0)] {
            let mut bytes = pcap(true, false, &[]);
            bytes[offset..offset + 4].copy_from_slice(&u32::to_le_bytes(value));
            assert!(Reader::new(bytes.as_slice()).await.is_err());
        }
        for (offset, value) in [(32, 65537), (32, 0), (36, 1), (28, 1_000_000)] {
            let mut bytes = pcap(true, false, &[frame()]);
            bytes[offset..offset + 4].copy_from_slice(&u32::to_le_bytes(value));
            let mut reader = Reader::new(bytes.as_slice()).await.unwrap();
            assert!(reader.next().await.is_err());
        }
        let bytes = pcap(true, false, &[frame()]);
        for length in [0, 4, 23] {
            assert!(Reader::new(&bytes[..length]).await.is_err());
        }
        for length in [25, 39, bytes.len() - 1] {
            let mut reader = Reader::new(&bytes[..length]).await.unwrap();
            assert!(reader.next().await.is_err());
        }
        assert!(Reader::new(&b"\x0a\x0d\x0d\x0a00000000000000000000"[..])
            .await
            .is_err());
    }
    #[tokio::test]
    async fn incomplete_capture_is_counted_and_blocked_header_is_cancellable() {
        let mut bytes = pcap(true, false, &[frame()]);
        let original = (frame().len() as u32 + 10).to_le_bytes();
        bytes[36..40].copy_from_slice(&original);
        let (tx, mut rx) = mpsc::channel(1);
        let control = ScanControl::new(true);
        let mut stats = Stats::default();
        assert!(consume(bytes.as_slice(), &tx, &control, &mut stats)
            .await
            .unwrap());
        assert_eq!((stats.records, stats.rejected, stats.events), (1, 1, 0));
        assert!(rx.try_recv().is_err());
        let (input, _writer) = tokio::io::duplex(32);
        let task_control = control.clone();
        let task =
            tokio::spawn(
                async move { consume(input, &tx, &task_control, &mut Stats::default()).await },
            );
        tokio::task::yield_now().await;
        control.shutdown();
        assert!(
            !tokio::time::timeout(std::time::Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
        );
    }

    #[tokio::test]
    async fn pause_backpressure_shutdown_and_rejection() {
        let bytes = pcap(true, false, &vec![frame(); 100]);
        let control = ScanControl::new(false);
        let (tx, mut rx) = mpsc::channel(1);
        let task_control = control.clone();
        let task = tokio::spawn(async move {
            let mut stats = Stats::default();
            consume(bytes.as_slice(), &tx, &task_control, &mut stats).await
        });
        tokio::task::yield_now().await;
        assert!(rx.try_recv().is_err());
        control.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(matches!(rx.recv().await, Some(ScanEvent::Replay { .. })));
        control.shutdown();
        assert!(
            !tokio::time::timeout(std::time::Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
        );
        let bytes = pcap(true, false, &[vec![0; 13]]);
        let control = ScanControl::new(true);
        let (tx, _rx) = mpsc::channel(1);
        let mut stats = Stats::default();
        assert!(consume(bytes.as_slice(), &tx, &control, &mut stats)
            .await
            .unwrap());
        assert_eq!(stats.rejected, 1);
    }
}
