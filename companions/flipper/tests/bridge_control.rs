use airhound_flipper::{
    bridge::{Command, Header, State},
    bridge_control::{Controller, UartAction},
};
fn h(id: u32, instance: u64) -> Header {
    Header {
        request_id: id,
        nonce: [7; 16],
        app_instance: instance,
        sequence: 0,
    }
}
fn ready(now: u32) -> Controller {
    let mut c = Controller::new(42, now);
    assert!(c.request(h(1, 0), Command::Hello, now).unwrap().hello);
    c
}
fn running(now: u32) -> Controller {
    let mut c = ready(now);
    assert_eq!(
        c.request(h(2, 42), Command::StartBeacon, now).unwrap().uart,
        Some(UartAction::Stop)
    );
    c.feed(now.wrapping_add(1), b"Stopping WiFi tran/recv\r\n");
    assert_eq!(c.poll(now.wrapping_add(51)), Some(UartAction::StartBeacon));
    assert_eq!(c.state, State::Preflight);
    assert_eq!(c.evidence, 1);
    c.feed(
        now.wrapping_add(52),
        b"StartingBeacon sniff. Stop with stopscan\r\n",
    );
    c.poll(now.wrapping_add(52));
    assert_eq!(c.state, State::Capturing);
    c
}
#[test]
fn hello_and_unbound_controls_never_start_uart() {
    let mut c = Controller::new(42, 0);
    assert!(c.request(h(1, 42), Command::StartBeacon, 0).is_err());
    assert_eq!(c.state, State::Ready);
    assert_eq!(c.request(h(1, 0), Command::Hello, 0).unwrap().uart, None);
    assert_eq!(c.request(h(2, 42), Command::Stop, 1).unwrap().uart, None);
    assert_eq!(c.state, State::Stopped);
    assert_eq!(c.evidence, 0);
    assert!(c.request(h(3, 42), Command::StartBeacon, 2).is_err());
}
#[test]
fn preflight_needs_exact_ack_quiet_and_complete_frame() {
    let mut c = ready(0);
    c.request(h(2, 42), Command::StartBeacon, 0).unwrap();
    c.feed(1, b"#\xf3stopscan\r\n> ");
    assert_eq!(c.poll(100), None);
    assert_eq!(c.evidence, 0);
    c.feed(110, b"\r\nStopping WiFi tran/recv\r\n[BUF/");
    assert_eq!(c.poll(500), None);
    assert_eq!(c.poll(1500), Some(UartAction::Stop));
    assert_eq!(c.cause, 7);
    assert_eq!(c.evidence & 2, 0);
    c.poll(2500);
    assert_eq!(c.state, State::Failed);
}
#[test]
fn duplicate_or_invalid_controls_cannot_refresh_watchdog() {
    let mut c = running(0);
    assert!(c.request(h(2, 42), Command::StartBeacon, 2900).is_err());
    let mut bad = h(3, 42);
    bad.nonce = [8; 16];
    assert!(c.request(bad, Command::Heartbeat, 2999).is_err());
    assert_eq!(c.poll(3000), Some(UartAction::Stop));
    assert_eq!(c.cause, 3);
    assert_eq!(c.request_id(), 0);
    c.feed(3001, b"Stopping WiFi tran/recv\r\n");
    c.poll(3100);
    assert_eq!(c.state, State::Failed);
    assert_eq!(c.evidence & 4, 4);
    assert!(c.request(h(4, 42), Command::StartBeacon, 3200).is_err());
}
#[test]
fn heartbeat_does_not_replace_lifecycle_request_and_clock_wraps() {
    let now = u32::MAX - 100;
    let mut c = running(now);
    assert_eq!(
        c.request(h(3, 42), Command::Heartbeat, now.wrapping_add(2500))
            .unwrap()
            .uart,
        None
    );
    assert_eq!(c.request_id(), 2);
    assert_eq!(c.poll(now.wrapping_add(5499)), None);
    assert_eq!(c.poll(now.wrapping_add(5500)), Some(UartAction::Stop));
}
#[test]
fn pause_resume_requires_new_preflight_generation_and_stop_ack() {
    let mut c = running(0);
    assert_eq!(
        c.request(h(3, 42), Command::Pause, 100).unwrap().uart,
        Some(UartAction::Stop)
    );
    assert_eq!(c.request_id(), 3);
    assert!(c.request(h(4, 42), Command::StartBeacon, 101).is_err());
    c.feed(102, b"Stopping WiFi tran/recv\r\n");
    c.poll(200);
    assert_eq!(c.state, State::Paused);
    assert_eq!(
        c.request(h(5, 42), Command::StartBeacon, 201).unwrap().uart,
        Some(UartAction::Stop)
    );
    assert_eq!(c.generation, 2);
    assert_eq!(c.evidence, 0);
    assert_eq!(c.poll(300), None);
    c.feed(301, b"Stopping WiFi tran/recv\r\n");
    assert_eq!(c.poll(351), Some(UartAction::StartBeacon));
}
#[test]
fn queue_failure_before_start_never_requires_sending_start() {
    let mut c = ready(0);
    c.request(h(2, 42), Command::StartBeacon, 0).unwrap();
    c.feed(1, b"Stopping WiFi tran/recv\r\n");
    assert_eq!(c.poll(51), Some(UartAction::StartBeacon)); // caller cannot enqueue pre-action status
    assert_eq!(c.fail(51, 8), Some(UartAction::Stop));
    assert_eq!(c.state, State::Stopping);
    assert_eq!(c.evidence & 2, 0);
    c.feed(52, b"Stopping WiFi tran/recv\r\n");
    c.poll(151);
    assert_eq!(c.state, State::Failed);
}
#[test]
fn loss_during_pause_teardown_cannot_become_clean_paused() {
    let mut c = running(0);
    c.request(h(3, 42), Command::Pause, 100).unwrap();
    assert_eq!(c.fail(101, 5), None);
    c.feed(102, b"Stopping WiFi tran/recv\r\n");
    c.poll(200);
    assert_eq!(c.state, State::Failed);
    assert_eq!(c.cause, 5);
}
#[test]
fn missing_ack_has_one_retry_then_bounded_unknown_failure() {
    let mut c = running(0);
    c.request(h(3, 42), Command::Stop, 100).unwrap();
    assert_eq!(c.poll(599), None);
    assert_eq!(c.poll(600), Some(UartAction::Stop));
    assert_eq!(c.poll(601), None);
    c.poll(1100);
    assert_eq!(c.state, State::Failed);
    assert_eq!(c.evidence & 4, 0);
    assert_eq!(c.cause, 10);
}
#[test]
fn final_transport_loss_after_stop_cannot_retain_clean_success() {
    let mut c = running(0);
    c.request(h(3, 42), Command::Stop, 100).unwrap();
    c.feed(101, b"Stopping WiFi tran/recv\r\n");
    c.poll(200);
    assert_eq!(c.state, State::Stopped);
    assert_eq!(c.fail(201, 5), None);
    assert_eq!(c.state, State::Failed);
    assert_eq!(c.evidence & 4, 4);
}
#[test]
fn finish_after_pause_needs_fresh_request_scoped_stop_evidence() {
    let mut c = running(0);
    c.request(h(3, 42), Command::Pause, 100).unwrap();
    c.feed(101, b"Stopping WiFi tran/recv\r\n");
    c.poll(200);
    assert_eq!(c.state, State::Paused);
    assert_eq!(c.evidence, 15);
    assert_eq!(
        c.request(h(4, 42), Command::Stop, 210).unwrap().uart,
        Some(UartAction::Stop)
    );
    assert_eq!(c.evidence, 3);
    assert_eq!(c.request_id(), 4);
    c.poll(310);
    assert_eq!(c.state, State::Stopping);
    c.feed(311, b"Stopping WiFi tran/recv\r\n");
    c.poll(361);
    assert_eq!(c.state, State::Stopped);
    assert_eq!(c.evidence, 15);
}
#[test]
fn pause_to_stop_escalation_and_repeated_stop_cannot_extend_deadline() {
    let mut c = running(0);
    c.request(h(3, 42), Command::Pause, 100).unwrap();
    assert_eq!(
        c.request(h(4, 42), Command::Stop, 900).unwrap().uart,
        Some(UartAction::Stop)
    );
    assert_eq!(c.request_id(), 4);
    assert_eq!(c.evidence, 3);
    assert!(c.request(h(5, 42), Command::Stop, 1099).is_err());
    c.poll(1900);
    assert_eq!(c.state, State::Failed);
    assert_eq!(c.cause, 10);
}

fn pcap_tail(bytes: usize) -> Vec<u8> {
    let mut data = vec![0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0];
    data.extend([0; 8]);
    data.extend(4096u32.to_le_bytes());
    data.extend(105u32.to_le_bytes());
    while data.len() < bytes {
        let n = (bytes - data.len()).saturating_sub(16).min(2000);
        // Reserve enough space for a following record header if needed.
        let n = if bytes - data.len() - 16 - n < 16 && bytes - data.len() - 16 != n {
            n - 16
        } else {
            n
        };
        for x in [1u32, 2, n as u32, n as u32] {
            data.extend(x.to_le_bytes());
        }
        data.extend(vec![0x80; n]);
    }
    let mut framed = b"[BUF/BEGIN]".to_vec();
    framed.extend(data);
    framed.extend(b"[BUF/CLOSE]");
    framed
}

#[test]
fn delayed_ack_and_valid_queued_tail_drain_within_fixed_bounds() {
    // Ordinary non-PSRAM source buffers: 2*3KiB. The optional 2*8KiB
    // PSRAM case is an upper-capacity stress, not this board's proven config.
    for bytes in [6 * 1024, 16 * 1024] {
        for origin in [0u32, u32::MAX - 900] {
            let mut c = running(origin);
            let stop = origin.wrapping_add(100);
            c.request(h(3, 42), Command::Pause, stop).unwrap();
            let ack = stop.wrapping_add(625);
            c.feed(ack, b"Stopping WiFi tran/recv\r\n");
            c.poll(ack);
            let tail = pcap_tail(bytes);
            for (i, chunk) in tail.chunks(115).enumerate() {
                // <=11,520 bytes/s (115200 baud, 8N1), including framing.
                let now = ack.wrapping_add((i as u32 + 1) * 10);
                c.feed(now, chunk);
                c.poll(now);
                assert_eq!(c.state, State::Stopping);
                if now.wrapping_sub(stop) >= 1000 {
                    // The prior shared 1s ack+drain deadline failed here even
                    // though this is a complete, finite valid queued stream.
                    assert_eq!(c.cause, 1);
                }
            }
            let finished = ack.wrapping_add(tail.len().div_ceil(115) as u32 * 10 + 50);
            c.poll(finished);
            assert_eq!(c.state, State::Paused);
            assert_eq!(c.evidence, 15);
            assert!(finished.wrapping_sub(stop) < 3000);
        }
    }
}

#[test]
fn incomplete_malformed_and_endless_tail_have_specific_bounded_failure() {
    for (tail, expected) in [
        (b"[BUF/BEGIN]".as_slice(), 11),
        (b"[BUF/BEGIN]xxxxxxxxxxxxxxxxxxxxxxxx".as_slice(), 6),
        (b"plain console\r\n".as_slice(), 12),
    ] {
        let mut c = running(0);
        c.request(h(3, 42), Command::Pause, 100).unwrap();
        c.feed(725, b"Stopping WiFi tran/recv\r\n");
        c.poll(725);
        c.feed(800, tail);
        // Keep recent input observable for the endless-but-well-framed case.
        if expected == 12 {
            c.pending_input(2725);
        }
        c.poll(2725);
        assert_eq!(c.state, State::Failed);
        assert_eq!(c.cause, expected);
        assert_eq!(c.evidence, 7);
        assert_eq!(c.request_id(), 3);
    }
}

#[test]
fn repeated_ack_heartbeat_and_stop_escalation_never_extend_absolute_bound() {
    let mut c = running(0);
    c.request(h(3, 42), Command::Pause, 100).unwrap();
    c.feed(900, b"Stopping WiFi tran/recv\r\n");
    c.poll(900);
    for now in [1400, 1900, 2400, 2800] {
        c.feed(now, b"Stopping WiFi tran/recv\r\n");
        c.poll(now);
        assert_eq!(c.state, State::Stopping);
    }
    c.request(h(4, 42), Command::Heartbeat, 2801).unwrap();
    c.request(h(5, 42), Command::Stop, 2899).unwrap();
    assert!(c.request(h(6, 42), Command::Stop, 2999).is_err());
    c.feed(3090, b"Stopping WiFi tran/recv\r\n");
    c.poll(3090);
    c.pending_input(3100);
    c.poll(3100);
    assert_eq!(c.state, State::Failed);
    assert_eq!(c.cause, 12);
    assert_eq!(c.request_id(), 5);
}

#[test]
fn late_ack_cannot_reopen_expired_ack_budget() {
    let mut c = running(0);
    c.request(h(3, 42), Command::Stop, 100).unwrap();
    c.feed(1100, b"Stopping WiFi tran/recv\r\n");
    c.poll(1150);
    assert_eq!(c.state, State::Failed);
    assert_eq!(c.cause, 10);
    assert_eq!(c.evidence, 7); // observed late text, not a valid bounded stop
}

#[test]
fn retry_reply_after_first_quiet_boundary_cannot_become_late_paused_input() {
    // Reproduce the observed timing, not the unknown contents of its 30 bytes.
    for origin in [0u32, u32::MAX - 3000] {
        let at = |n: u32| origin.wrapping_add(n);
        let mut c = running(origin);
        c.request(h(3, 42), Command::Pause, at(2300)).unwrap();
        assert_eq!(c.poll(at(2800)), Some(UartAction::Stop));
        c.feed(at(3036), b"Stopping WiFi tran/recv\r\n");
        c.poll(at(3036));
        c.feed(at(3436), &pcap_tail(100));
        c.poll(at(3486));
        assert_eq!(c.state, State::Stopping); // Old code prematurely Paused.
        c.feed(at(3492), b"#stopscan\r\nStopping WiFi ");
        c.poll(at(3542));
        assert_eq!(c.state, State::Stopping); // A partial reply is no credit.
        c.feed(at(3550), b"tran/recv\r\n> ");
        c.poll(at(3599));
        assert_eq!(c.state, State::Stopping);
        c.poll(at(3600));
        assert_eq!(c.state, State::Paused);
        assert_eq!(c.evidence, 15);
        // A duplicate completed reply observed before a later Stop is historical.
        // Test the controller's accounting only; runtime rejects late Paused RX.
        c.feed(at(3601), b"\r\nStopping WiFi tran/recv\r\n");
        c.request(h(4, 42), Command::Stop, at(3610)).unwrap();
        c.poll(at(3710));
        assert_eq!(c.state, State::Stopping);
        assert_eq!(c.evidence & 4, 0);
        c.feed(at(3711), b"Stopping WiFi tran/recv\r\n");
        c.poll(at(3761));
        assert_eq!(c.state, State::Stopped);
    }
}

#[test]
fn missing_retry_reply_fails_at_original_fixed_drain_deadline() {
    let mut c = running(0);
    c.request(h(3, 42), Command::Pause, 100).unwrap();
    assert_eq!(c.poll(600), Some(UartAction::Stop));
    c.feed(725, b"Stopping WiFi tran/recv\r\n");
    c.poll(725);
    c.poll(775);
    assert_eq!(c.state, State::Stopping);
    c.request(h(4, 42), Command::Heartbeat, 2700).unwrap();
    c.poll(2724);
    assert_eq!(c.state, State::Stopping);
    c.poll(2725);
    assert_eq!(c.state, State::Failed);
    assert_eq!(c.cause, 14);
    assert_eq!(c.evidence & 4, 4); // One reply observed; completion still failed.
}

#[test]
fn escalation_carries_outstanding_retries_and_needs_its_own_reply() {
    for finish_reply in [false, true] {
        let mut c = running(0);
        c.request(h(3, 42), Command::Pause, 100).unwrap();
        assert_eq!(c.poll(600), Some(UartAction::Stop));
        c.feed(725, b"Stopping WiFi tran/recv\r\n");
        c.poll(725);
        c.request(h(4, 42), Command::Stop, 800).unwrap();
        assert_eq!(c.evidence & 4, 0);
        // This reply can belong to the old retry. It cannot alone complete Finish.
        c.feed(850, b"Stopping WiFi tran/recv\r\n");
        c.poll(850);
        c.poll(900);
        assert_eq!(c.state, State::Stopping);
        assert!(c.request(h(5, 42), Command::Stop, 901).is_err());
        if finish_reply {
            c.feed(950, b"Stopping WiFi tran/recv\r\n");
            c.poll(1000);
            assert_eq!(c.state, State::Stopped);
        } else {
            c.request(h(6, 42), Command::Heartbeat, 2800).unwrap();
            c.poll(2850);
            assert_eq!(c.state, State::Failed);
            assert_eq!(c.cause, 14);
        }
    }
}

#[test]
fn cancelled_preflight_cannot_credit_old_stop_reply_to_new_stop() {
    let mut c = ready(0);
    c.request(h(2, 42), Command::StartBeacon, 0).unwrap();
    c.request(h(3, 42), Command::Stop, 100).unwrap();
    c.feed(101, b"Stopping WiFi tran/recv\r\n");
    c.poll(200);
    assert_eq!(c.state, State::Stopping);
    assert_eq!(c.evidence & 2, 0);
    c.feed(201, b"Stopping WiFi tran/recv\r\n");
    c.poll(251);
    assert_eq!(c.state, State::Stopped);
    assert_eq!(c.evidence & 2, 0); // No Start ever issued.
}

#[test]
fn autonomous_terminal_completion_distinguishes_identical_old_failure_tuples() {
    for origin in [0u32, u32::MAX - 4000] {
        let at = |n: u32| origin.wrapping_add(n);
        // All cases previously ended Failed/cause3/evidence7. Only the clean
        // all-replies + quiet case may now carry explicit completion bit3.
        for case in ["clean", "missing_retry", "noisy", "partial", "malformed"] {
            let mut c = running(origin);
            assert_eq!(c.poll(at(3000)), Some(UartAction::Stop));
            assert_eq!(c.poll(at(3500)), Some(UartAction::Stop));
            c.feed(at(3600), b"Stopping WiFi tran/recv\r\n");
            c.poll(at(3600));
            if case != "missing_retry" {
                c.feed(at(3700), b"Stopping WiFi tran/recv\r\n");
                c.poll(at(3700));
            }
            match case {
                "partial" => c.feed(at(3701), b"[BUF/BEGIN]"),
                "malformed" => c.feed(at(3701), b"[BUF/BEGIN]xxxxxxxxxxxxxxxxxxxxxxxx"),
                "noisy" => c.pending_input(at(5600)),
                _ => {}
            }
            c.poll(at(if case == "clean" { 3750 } else { 5600 }));
            assert_eq!(c.state, State::Failed, "{case}");
            assert_eq!(c.cause, 3, "{case}");
            assert_eq!(c.evidence & 7, 7, "old tuple must match: {case}");
            assert_eq!(
                c.evidence & 8,
                if case == "clean" { 8 } else { 0 },
                "{case}"
            );
        }
    }
}

#[test]
fn final_loss_revokes_completed_autonomous_failure_and_cannot_be_relabelled() {
    let mut c = running(0);
    c.poll(3000);
    c.feed(3001, b"Stopping WiFi tran/recv\r\n");
    c.poll(3100);
    assert_eq!((c.state, c.cause, c.evidence), (State::Failed, 3, 15));
    c.fail(3101, 5); // final ISR counters after the terminal poll
    assert_eq!((c.state, c.cause, c.evidence), (State::Failed, 3, 7));
    let mut c = running(0);
    c.fail(100, 5);
    c.fail(101, 4); // a later transport cause cannot hide earlier known loss
    c.feed(102, b"Stopping WiFi tran/recv\r\n");
    c.poll(200);
    assert_eq!((c.state, c.evidence & 8), (State::Failed, 0));
}
