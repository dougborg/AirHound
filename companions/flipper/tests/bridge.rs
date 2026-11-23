use airhound_flipper::bridge::*;

fn header() -> Header {
    Header {
        request_id: 1,
        nonce: [0x42; 16],
        app_instance: 0,
        sequence: 0,
    }
}
fn hello() -> Hello<'static> {
    Hello {
        capabilities: CAPABILITIES,
        api_major: 87,
        api_minor: 1,
        uart_baud: 115200,
        watchdog_ms: 3000,
        app_version: "0.1.0",
        source_revision: "0123456789abcdef0123456789abcdef01234567",
        source_modified: 2,
    }
}
fn hello_message() -> Message<'static> {
    Message {
        header: Header {
            app_instance: 0x0123456789abcdef,
            sequence: 1,
            ..header()
        },
        body: Body::Hello(hello()),
    }
}

#[test]
fn fixed_header_golden_and_all_message_payloads_roundtrip() {
    let mut out = [0; MAX_MESSAGE];
    let request = Message {
        header: header(),
        body: Body::Command(Command::Hello),
    };
    assert_eq!(encode(request, &mut out).unwrap(), 44);
    assert_eq!(&out[..12], b"AHFB\x01\x01\x00\x00\x01\x00\x00\x00");
    assert_eq!(&out[12..28], &[0x42; 16]);
    assert_eq!(&out[28..44], &[0; 16]);
    assert_eq!(decode(&out[..44]).unwrap(), request);
    let raw = [0xab; 248];
    let status = Status {
        state: State::Capturing,
        cause: 0,
        evidence: 3,
        generation: u32::MAX,
        app_uptime_ms: u32::MAX,
        uart_rx_bytes: u64::MAX,
        uart_dropped_bytes: 5,
        uart_error_events: 6,
        rpc_dropped_messages: 7,
        unread_ring_bytes: u32::MAX,
    };
    for body in [
        Body::Hello(hello()),
        Body::Ack {
            command: Command::StartBeacon,
            result: 0,
        },
        Body::Status(status),
        Body::Raw {
            generation: u32::MAX,
            app_uptime_ms: 42,
            bytes: &raw,
        },
    ] {
        let h = Header {
            request_id: if matches!(body, Body::Raw { .. }) {
                0
            } else {
                1
            },
            ..hello_message().header
        };
        let message = Message { header: h, body };
        let len = encode(message, &mut out).unwrap();
        assert_eq!(decode(&out[..len]).unwrap(), message);
        if matches!(body, Body::Raw { .. }) {
            assert_eq!(len, 300)
        }
        if matches!(body, Body::Status(_)) {
            assert_eq!(len, 92)
        }
        for shortened in 0..len {
            assert!(decode(&out[..shortened]).is_err());
        }
        if len < MAX_MESSAGE {
            assert!(decode(&out[..len + 1]).is_err());
        }
    }
}

#[test]
fn invalid_metadata_and_reserved_fields_fail_closed() {
    let mut out = [0; MAX_MESSAGE];
    let len = encode(hello_message(), &mut out).unwrap();
    for offset in [0, 4, 5, 6, 7, 40, 41, 42, 43] {
        let mut bad = out;
        bad[offset] = 0xff;
        assert!(decode(&bad[..len]).is_err(), "offset {offset}");
    }
    for h in [
        Hello {
            app_version: "\n",
            ..hello()
        },
        Hello {
            source_revision: "bad",
            ..hello()
        },
        Hello {
            source_modified: 3,
            ..hello()
        },
        Hello {
            capabilities: 16,
            ..hello()
        },
        Hello {
            watchdog_ms: 0,
            ..hello()
        },
        Hello {
            uart_baud: 9600,
            ..hello()
        },
    ] {
        assert!(
            encode(
                Message {
                    body: Body::Hello(h),
                    ..hello_message()
                },
                &mut out
            )
            .is_err()
        );
    }
    assert!(
        encode(
            Message {
                header: Header {
                    nonce: [0; 16],
                    ..header()
                },
                body: Body::Command(Command::Hello)
            },
            &mut out
        )
        .is_err()
    );
    assert!(
        encode(
            Message {
                header: header(),
                body: Body::Command(Command::StartBeacon)
            },
            &mut out
        )
        .is_err()
    );
    assert!(
        encode(
            Message {
                header: Header {
                    app_instance: 1,
                    ..header()
                },
                body: Body::Command(Command::Hello)
            },
            &mut out
        )
        .is_err()
    );
}

#[test]
fn binding_rejects_wrong_nonce_instance_stale_sequence_and_gap() {
    let first = hello_message();
    let mut b = Binding::new(header().nonce, 1).unwrap();
    b.accept(first).unwrap();
    assert_eq!(b.instance(), first.header.app_instance);
    let next = Message {
        header: Header {
            sequence: 2,
            ..first.header
        },
        body: Body::Ack {
            command: Command::Heartbeat,
            result: 0,
        },
    };
    for bad in [
        first,
        Message {
            header: Header {
                sequence: 3,
                ..next.header
            },
            ..next
        },
        Message {
            header: Header {
                nonce: [1; 16],
                ..next.header
            },
            ..next
        },
        Message {
            header: Header {
                app_instance: 1,
                ..next.header
            },
            ..next
        },
    ] {
        let mut b = Binding::new(header().nonce, 1).unwrap();
        b.accept(first).unwrap();
        assert!(b.accept(bad).is_err());
        assert!(b.accept(next).is_err());
    }
    b.accept(next).unwrap();
    assert!(Binding::new([0; 16], 1).is_err());
    assert!(Binding::new([1; 16], 0).is_err());
}

#[test]
fn stop_failure_causes_roundtrip_and_unknown_causes_fail_closed() {
    let mut out = [0; MAX_MESSAGE];
    let mut status = Status {
        state: State::Failed,
        cause: 10,
        evidence: 7,
        generation: 1,
        app_uptime_ms: 3000,
        uart_rx_bytes: 5000,
        uart_dropped_bytes: 0,
        uart_error_events: 0,
        rpc_dropped_messages: 0,
        unread_ring_bytes: 0,
    };
    for cause in 10..=14 {
        status.cause = cause;
        let message = Message {
            header: hello_message().header,
            body: Body::Status(status),
        };
        let len = encode(message, &mut out).unwrap();
        assert_eq!(decode(&out[..len]).unwrap(), message);
        out[45] = 15;
        assert!(decode(&out[..len]).is_err());
    }
}

#[test]
fn completion_capability_and_evidence_are_explicit_and_bounded() {
    assert_eq!(CAPABILITIES, 15);
    let mut out = [0; MAX_MESSAGE];
    let n = encode(hello_message(), &mut out).unwrap();
    assert_eq!(u32::from_le_bytes(out[44..48].try_into().unwrap()), 15);
    assert_eq!(decode(&out[..n]).unwrap(), hello_message());
    let mut status = Status {
        state: State::Failed,
        cause: 3,
        evidence: 15,
        generation: 1,
        app_uptime_ms: 3100,
        uart_rx_bytes: 100,
        uart_dropped_bytes: 0,
        uart_error_events: 0,
        rpc_dropped_messages: 0,
        unread_ring_bytes: 0,
    };
    let message = Message {
        header: hello_message().header,
        body: Body::Status(status),
    };
    let n = encode(message, &mut out).unwrap();
    assert_eq!(decode(&out[..n]).unwrap(), message);
    status.evidence = 16;
    assert!(
        encode(
            Message {
                body: Body::Status(status),
                ..message
            },
            &mut out
        )
        .is_err()
    );
}
