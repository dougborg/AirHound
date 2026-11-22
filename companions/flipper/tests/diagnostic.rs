use airhound_flipper::diagnostic::{Command, Phase, Run, Witness, write_complete};

fn global() -> Vec<u8> {
    let mut g = vec![0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0];
    g.extend([0; 8]);
    g.extend(4096u32.to_le_bytes());
    g.extend(105u32.to_le_bytes());
    g
}
fn packet(payload: &[u8]) -> Vec<u8> {
    let mut p = Vec::new();
    for n in [3u32, 42, payload.len() as u32, payload.len() as u32] {
        p.extend(n.to_le_bytes());
    }
    p.extend(payload);
    p
}
fn block(data: &[u8]) -> Vec<u8> {
    let mut b = b"[BUF/BEGIN]".to_vec();
    b.extend(data);
    b.extend(b"[BUF/CLOSE]");
    b
}
fn start(now: u32) -> Run {
    let mut r = Run::new(now);
    r.feed(now.wrapping_add(1), b"Stopping WiFi tran/recv\r\n");
    assert_eq!(
        r.poll(now.wrapping_add(51), false, false),
        Some(Command::Start)
    );
    r
}
#[test]
fn split_markers_and_binary_ack_collisions_never_change_console_witness() {
    let payload=b"\nStopping WiFi tran/recv\nStartingBeacon sniff. Stop with stopscan\n[BUF/CLOSE][BUF/BEGIN]";
    let mut stream = block(&global());
    stream.extend(block(&packet(payload)));
    stream.extend(b"StartingBeacon sniff. Stop with stopscan\r\nStopping WiFi tran/recv\r\n");
    for chunk_size in 1..=stream.len() {
        let mut r = Witness::new();
        for chunk in stream.chunks(chunk_size) {
            for &b in chunk {
                r.feed(b);
            }
        }
        assert!(!r.malformed);
        assert!(r.outside());
        assert_eq!((r.starts, r.stops), (1, 1));
    }
}
#[test]
fn only_complete_exact_lines_count_and_long_lines_do_not() {
    let mut w = Witness::new();
    for &b in
        b"prefix Stopping WiFi tran/recv\nStopping WiFi tran/recv suffix\nStopping WiFi tran/recv"
    {
        w.feed(b);
    }
    assert_eq!(w.stops, 0);
    w.feed(b'\n');
    assert_eq!(w.stops, 1);
    for _ in 0..256 {
        w.feed(b'x');
    }
    for &b in b"Stopping WiFi tran/recv\n" {
        w.feed(b);
    }
    assert_eq!(w.stops, 1);
}
#[test]
fn malformed_or_incomplete_frame_cannot_supply_text_ack() {
    for prefix in [
        b"[BUF/BEGIN]".as_slice(),
        b"[BUF/BEGIN]bad header".as_slice(),
    ] {
        let mut w = Witness::new();
        for &b in prefix.iter().chain(b"Stopping WiFi tran/recv\n") {
            w.feed(b);
        }
        assert_eq!(w.stops, 0);
        assert!(!w.outside());
    }
    let mut w = Witness::new();
    let mut p = packet(b"a");
    p[8..12].copy_from_slice(&5000u32.to_le_bytes());
    for b in block(&global()).into_iter().chain(block(&p)) {
        w.feed(b);
    }
    assert!(w.malformed);
}
#[test]
fn stop_ack_before_stop_command_does_not_end_run() {
    let mut r = start(0);
    r.feed(
        100,
        b"StartingBeacon sniff. Stop with stopscan\nStopping WiFi tran/recv\n",
    );
    assert_eq!(r.poll(2051, false, false), Some(Command::Stop));
    assert!(r.evidence.start_text_ack);
    assert_eq!(r.poll(2551, false, false), Some(Command::Stop));
    r.poll(3051, false, false);
    assert_eq!(r.phase, Phase::Done);
    assert!(!r.evidence.stop_text_ack);
}
#[test]
fn local_deadline_wraps_and_stop_drains_without_more_commands() {
    let origin = u32::MAX - 100;
    let mut r = start(origin);
    let began = origin.wrapping_add(51);
    assert_eq!(r.poll(began.wrapping_add(1999), false, false), None);
    assert_eq!(
        r.poll(began.wrapping_add(2000), false, false),
        Some(Command::Stop)
    );
    assert_eq!(r.evidence.stop_after_ms, 2000);
    r.feed(began.wrapping_add(2010), b"Stopping WiFi tran/recv\r\n");
    r.poll(began.wrapping_add(2050), false, false);
    assert_eq!(r.phase, Phase::Stopping);
    r.poll(began.wrapping_add(2100), false, false);
    assert_eq!(r.phase, Phase::Done);
    assert!(r.evidence.stop_text_ack);
    assert_eq!(r.poll(began.wrapping_add(5000), false, false), None);
}
#[test]
fn preflight_requires_ack_and_aborts_on_cancel_loss_or_deadline() {
    for (cancel, loss) in [(true, false), (false, true)] {
        let mut r = Run::new(0);
        assert_eq!(r.poll(1, cancel, loss), None);
        assert_eq!(r.phase, Phase::Done);
        assert!(!r.evidence.start_sent);
    }
    let mut r = Run::new(0);
    r.poll(1500, false, false);
    assert_eq!(r.phase, Phase::Done);
}
#[test]
fn overflow_or_cancel_sends_stop_immediately_then_has_bounded_unknown_exit() {
    for (cancel, loss) in [(true, false), (false, true)] {
        let mut r = start(0);
        assert_eq!(r.poll(75, cancel, loss), Some(Command::Stop));
        assert_eq!(r.evidence.stop_after_ms, 24);
        assert_eq!(r.poll(575, false, false), Some(Command::Stop));
        r.poll(1075, false, false);
        assert_eq!(r.phase, Phase::Done);
        assert!(!r.evidence.stop_text_ack);
    }
}
#[test]
fn short_writes_retry_but_zero_or_invalid_lengths_fail() {
    let mut out: Vec<u8> = Vec::new();
    assert!(write_complete(b"abcdef", |p| {
        let n = p.len().min(2);
        out.extend(&p[..n]);
        n
    }));
    assert_eq!(out, b"abcdef");
    assert!(!write_complete(b"abc", |_| 0));
    assert!(!write_complete(b"abc", |p| p.len() + 1));
}

#[test]
fn persistence_attempts_close_after_write_failure_and_reports_close_failure() {
    use airhound_flipper::diagnostic::{Output, persist};
    struct Sink {
        writes: usize,
        closes: usize,
        fail_write: bool,
        fail_close: bool,
    }
    impl Output for Sink {
        fn write(&mut self, b: &[u8]) -> usize {
            self.writes += 1;
            if self.fail_write { 0 } else { b.len().min(2) }
        }
        fn close(&mut self) -> bool {
            self.closes += 1;
            !self.fail_close
        }
    }
    for (bad_write, bad_close) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut sink = Sink {
            writes: 0,
            closes: 0,
            fail_write: bad_write,
            fail_close: bad_close,
        };
        let result = persist(b"abcdef", &mut sink);
        assert_eq!(result.written, !bad_write);
        assert_eq!(result.closed, !bad_close);
        assert_eq!(sink.closes, 1);
    }
}
#[test]
fn valid_header_only_is_not_packet_reception_and_truncated_packet_stays_incomplete() {
    let mut w = Witness::new();
    for b in block(&global()) {
        w.feed(b);
    }
    assert!(w.outside());
    assert_eq!(w.packets, 0);
    let p = packet(b"abcd");
    for &b in b"[BUF/BEGIN]".iter().chain(&p[..p.len() - 1]) {
        w.feed(b);
    }
    assert!(!w.outside());
    assert_eq!(w.packets, 0);
    w.feed(b'd');
    for &b in b"[BUF/CLOSE]" {
        w.feed(b);
    }
    assert!(w.outside());
    assert_eq!(w.packets, 1);
}
#[test]
fn late_transport_loss_after_done_never_becomes_ready() {
    let mut r = start(0);
    r.feed(100, b"StartingBeacon sniff. Stop with stopscan\n");
    r.feed(200, &block(&global()));
    r.feed(300, &block(&packet(b"abc")));
    assert_eq!(r.poll(2051, false, false), Some(Command::Stop));
    r.feed(2060, b"Stopping WiFi tran/recv\n");
    r.poll(2151, false, false);
    assert!(r.raw_ready(0, 0, 0));
    assert!(!r.raw_ready(1, 0, 0));
    assert!(!r.raw_ready(0, 1, 0));
    // Model a successfully queued ISR byte after final poll but before RX stop.
    // Neither counter increments, yet discarding that tail is still incomplete.
    assert!(!r.raw_ready(0, 0, 1));
}

#[test]
fn packet_must_fit_the_declared_global_snaplen() {
    let mut g = global();
    g[16..20].copy_from_slice(&1514u32.to_le_bytes());
    let payload = vec![42; 1515];
    let mut w = Witness::new();
    for b in block(&g).into_iter().chain(block(&packet(&payload))) {
        w.feed(b);
    }
    assert!(w.malformed);
    assert_eq!(w.packets, 0);
}

#[test]
fn default_device_selftest_runs_synthetic_framing_and_deadlines_only() {
    assert!(airhound_flipper::diagnostic::synthetic_self_test());
}

#[test]
fn preflight_only_retains_receive_evidence_but_can_never_start() {
    for ack in [false, true] {
        let mut run = Run::preflight(0);
        assert!(run.retain_bytes());
        if ack {
            run.feed(1, b"Stopping WiFi tran/recv\r\n");
        }
        for now in [0, 50, 51, 100, 1499, 1500, 3000] {
            assert_ne!(run.poll(now, false, false), Some(Command::Start));
        }
        assert_eq!(run.phase, Phase::Done);
        assert!(!run.evidence.start_sent);
        assert_eq!(run.preflight_ready(0, 0, 0), ack);
        assert!(!run.raw_ready(0, 0, 0));
        assert!(!run.preflight_ready(0, 0, 1));
    }
}

#[test]
fn preflight_ack_followed_by_incomplete_frame_never_starts_or_claims_complete() {
    for mut run in [Run::new(0), Run::preflight(0)] {
        run.feed(1, b"Stopping WiFi tran/recv\r\n[BUF/BE");
        assert_eq!(run.poll(100, false, false), None);
        assert_eq!(run.phase, Phase::Preflight);
        assert_eq!(run.poll(1500, false, false), None);
        assert_eq!(run.phase, Phase::Done);
        assert!(run.evidence.preflight_stop_text_ack);
        assert!(!run.evidence.start_sent);
        assert!(!run.preflight_ready(0, 0, 0));
    }
}
