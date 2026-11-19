//! Opt-in release-mode measurement of the production event pipeline.
//! Run: cargo test --manifest-path hostd/Cargo.toml --release performance -- --ignored --nocapture

use super::*;
use airhound::scanner::{BleAdvParser, FrameType, WiFiEvent};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "performance measurement; requires a release build"]
#[allow(clippy::assertions_on_constants)]
async fn performance_event_pipeline() {
    // An ignored benchmark must fail clearly if invoked in a debug build.
    assert!(
        !cfg!(debug_assertions),
        "run this measurement with --release"
    );
    let count = 100_000;
    let directory = std::env::temp_dir().join(format!("airhound-perf-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let mut ble = BleAdvParser::parse(&[0x12; 6], -45, &[]);
    ble.name.push_str("FS Ext Battery").unwrap();
    let wifi = WiFiEvent {
        mac: [0x34; 6],
        ssid: "Home".try_into().unwrap(),
        rssi: -50,
        channel: 6,
        frame_type: FrameType::Beacon,
        raw_ies: Default::default(),
        security: Default::default(),
    };
    for scenario in ["hardware_mac", "platform_id"] {
        for run in 0..3 {
            let csv_path = directory.join("observations.csv");
            let json_path = directory.join("events.ndjson");
            let csv = WigleWriter::new(std::fs::File::create(&csv_path).unwrap()).unwrap();
            let output = std::fs::File::create(&json_path).unwrap();
            let (tx, rx) = mpsc::channel(32);
            let started = Instant::now();
            let consumer = tokio::spawn(filter_task(
                rx,
                Arc::new(Mutex::new(FilterConfig::new())),
                Arc::new(MacIndex::from_defaults()),
                started,
                csv,
                output,
                ScanControl::new(true),
            ));
            // 20% matching BLE, 80% unmatched WiFi; CSV retains every observation.
            for index in 0..count {
                let event = if index % 5 == 0 {
                    if scenario == "platform_id" {
                        ScanEvent::BlePeripheral {
                            event: ble.clone(),
                            id: "00112233-4455-6677-8899-aabbccddeeff".into(),
                            address: None,
                        }
                    } else {
                        ScanEvent::Ble(ble.clone())
                    }
                } else {
                    ScanEvent::WiFi(wifi.clone())
                };
                tx.send(event).await.unwrap();
            }
            drop(tx);
            consumer.await.unwrap().unwrap();
            let elapsed = started.elapsed().as_secs_f64();
            // Validate output after stopping the clock; never time an empty workload.
            let csv_rows = io::BufReader::new(std::fs::File::open(&csv_path).unwrap())
                .lines()
                .count();
            let messages = io::BufReader::new(std::fs::File::open(&json_path).unwrap())
                .lines()
                .count();
            assert_eq!(
                csv_rows,
                if scenario == "platform_id" {
                    count * 4 / 5 + 2
                } else {
                    count + 2
                }
            );
            assert_eq!(messages, count / 5);
            println!("PERF scenario={scenario} run={run} events={count} matches={messages} elapsed_s={elapsed:.6} events_per_s={:.0} csv_bytes={} ndjson_bytes={}",
            count as f64 / elapsed, std::fs::metadata(&csv_path).unwrap().len(), std::fs::metadata(&json_path).unwrap().len());
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
