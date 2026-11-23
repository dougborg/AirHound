//! Bounded line formatting and incremental UART output, without allocation.
//!
//! The hardware adapter must serialize `ready` and `write` against every other
//! UART writer. Neither operation may wait for FIFO space. A blocked step leaves
//! the cursor unchanged so the caller can yield with interrupts enabled.

use core::fmt::{self, Write};

use crate::protocol::MsgBuffer;

pub const FIFO_CHUNK: usize = 16;
pub const LINE_BUDGET_US: u64 = 20_000;

struct Line(MsgBuffer);

impl Write for Line {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        // A diagnostic is exactly one line. The installed host's legacy parser
        // searches for the first '{' even after a log prefix, so escape opening
        // braces as text as well as flattening CR/LF. A prefix alone is unsafe.
        for byte in text.bytes() {
            if byte == b'{' {
                self.0.extend_from_slice(b"\\x7b").map_err(|_| fmt::Error)?;
                continue;
            }
            self.0
                .push(if matches!(byte, b'\r' | b'\n') {
                    b' '
                } else {
                    byte
                })
                .map_err(|_| fmt::Error)?;
        }
        Ok(())
    }
}

pub fn diagnostic(level: log::Level, args: fmt::Arguments<'_>) -> Result<MsgBuffer, fmt::Error> {
    let mut line = Line(MsgBuffer::new());
    write!(line, "{level} - {args}")?;
    line.0.push(b'\n').map_err(|_| fmt::Error)?;
    Ok(line.0)
}

pub fn is_observation(line: &[u8]) -> bool {
    line.first() == Some(&b'{')
}

#[derive(Debug, PartialEq, Eq)]
pub enum Enqueue {
    Queued,
    Full,
    FormatError,
}

pub fn enqueue_diagnostic(
    level: log::Level,
    args: fmt::Arguments<'_>,
    enqueue: impl FnOnce(MsgBuffer) -> bool,
) -> Enqueue {
    match diagnostic(level, args) {
        Ok(line) => {
            if enqueue(line) {
                Enqueue::Queued
            } else {
                Enqueue::Full
            }
        }
        Err(_) => Enqueue::FormatError,
    }
}

pub trait Fifo {
    type Error;
    fn ready(&mut self) -> bool;
    /// Write a nonempty slice after ready returned true, without waiting.
    fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Blocked,
    Progress,
    Complete,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DrainError<E> {
    Write(E),
    InvalidProgress,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LineError<E> {
    Timeout,
    Output(E),
}

#[derive(Default)]
pub struct Writer {
    failed: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WriteOutcome {
    Sent,
    Failed,
    Disabled,
}

impl Writer {
    pub fn write<E>(
        &mut self,
        line: &[u8],
        output_step: impl FnMut(&[u8], &mut usize) -> Result<Step, E>,
        now_micros: impl FnMut() -> u64,
    ) -> WriteOutcome {
        if self.failed {
            return WriteOutcome::Disabled;
        }
        if drain(line, output_step, now_micros).is_err() {
            self.failed = true;
            WriteOutcome::Failed
        } else {
            WriteOutcome::Sent
        }
    }
}

/// Bounded polling, not an asynchronous task operation. The adapter must leave
/// interrupts enabled between steps. The deadline never renews on progress.
/// At 460800/8N1 a maximum 512-byte line takes about 11.1ms on the wire; 20ms
/// allows FIFO contention/interrupt service, while bounding cooperative delay.
pub fn drain<E>(
    line: &[u8],
    mut output_step: impl FnMut(&[u8], &mut usize) -> Result<Step, E>,
    mut now_micros: impl FnMut() -> u64,
) -> Result<(), LineError<E>> {
    let started = now_micros();
    let mut offset = 0;
    loop {
        if now_micros().wrapping_sub(started) >= LINE_BUDGET_US {
            return Err(LineError::Timeout);
        }
        match output_step(line, &mut offset).map_err(LineError::Output)? {
            Step::Complete => return Ok(()),
            Step::Progress => {}
            Step::Blocked => core::hint::spin_loop(),
        }
    }
}

/// Perform at most one bounded FIFO write. No loop, sleep, or flush occurs here.
pub fn step<F: Fifo>(
    fifo: &mut F,
    line: &[u8],
    offset: &mut usize,
) -> Result<Step, DrainError<F::Error>> {
    if *offset == line.len() {
        return Ok(Step::Complete);
    }
    if *offset > line.len() {
        return Err(DrainError::InvalidProgress);
    }
    if !fifo.ready() {
        return Ok(Step::Blocked);
    }
    let end = (*offset + FIFO_CHUNK).min(line.len());
    let count = fifo.write(&line[*offset..end]).map_err(DrainError::Write)?;
    if count == 0 || count > end - *offset {
        return Err(DrainError::InvalidProgress);
    }
    *offset += count;
    Ok(if *offset == line.len() {
        Step::Complete
    } else {
        Step::Progress
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    #[derive(Default)]
    struct Sink {
        space: usize,
        bytes: Vec<u8>,
        writes: usize,
        fault: bool,
    }
    impl Fifo for Sink {
        type Error = ();
        fn ready(&mut self) -> bool {
            self.space > 0
        }
        fn write(&mut self, bytes: &[u8]) -> Result<usize, ()> {
            assert!(self.space > 0);
            assert!(bytes.len() <= FIFO_CHUNK);
            self.writes += 1;
            if self.fault {
                return Err(());
            }
            let n = self.space.min(bytes.len());
            self.bytes.extend_from_slice(&bytes[..n]);
            self.space -= n;
            Ok(n)
        }
    }

    #[test]
    fn blocked_fifo_never_writes_or_advances() {
        let mut sink = Sink::default();
        let mut cursor = 0;
        for _ in 0..1000 {
            assert_eq!(step(&mut sink, b"record\n", &mut cursor), Ok(Step::Blocked));
        }
        assert_eq!((cursor, sink.writes), (0, 0));
    }

    #[test]
    fn partial_writes_keep_lines_ordered_and_complete() {
        let log = diagnostic(
            log::Level::Info,
            format_args!("Button A\n{{\"fake\":1}}\rready"),
        )
        .unwrap();
        assert!(!is_observation(&log));
        let observation = b"{\"type\":\"ble_scan\",\"value\":123456789}\n";
        assert!(is_observation(observation));
        let mut sink = Sink::default();
        for line in [
            observation.as_slice(),
            log.as_slice(),
            observation.as_slice(),
        ] {
            let mut cursor = 0;
            loop {
                sink.space = 3;
                if step(&mut sink, line, &mut cursor).unwrap() == Step::Complete {
                    break;
                }
                assert_eq!(step(&mut sink, line, &mut cursor), Ok(Step::Blocked));
            }
        }
        assert_eq!(
            sink.bytes,
            [
                observation.as_slice(),
                log.as_slice(),
                observation.as_slice()
            ]
            .concat()
        );
        assert_eq!(sink.bytes.iter().filter(|&&b| b == b'\n').count(), 3);
    }

    #[test]
    fn overlong_diagnostic_is_rejected_instead_of_emitting_a_prefix() {
        assert!(diagnostic(log::Level::Warn, format_args!("{}", "x".repeat(512))).is_err());
        let line = diagnostic(log::Level::Info, format_args!("{}", "x".repeat(504))).unwrap();
        assert_eq!(line.len(), 512);
        assert_eq!(line.last(), Some(&b'\n'));
    }

    #[test]
    fn json_in_diagnostic_cannot_enter_legacy_host_json_parser() {
        let valid = "{\"type\":\"wifi_scan\",\"mac\":\"00:11:22:33:44:55\"}";
        let line = diagnostic(log::Level::Warn, format_args!("device reported:\n{valid}")).unwrap();
        // hostd/device_stream.rs searches for this byte anywhere in a line,
        // including after INFO/WARN prefixes. The diagnostic exposes none.
        assert!(!line.contains(&b'{'));
        assert_eq!(line.iter().filter(|&&b| b == b'\n').count(), 1);
        assert!(core::str::from_utf8(&line).unwrap().contains("\\x7b"));
        assert!(diagnostic(log::Level::Info, format_args!("{}", "{".repeat(128))).is_err());
    }

    #[test]
    fn write_failure_does_not_credit_unsent_bytes() {
        let mut sink = Sink {
            space: 16,
            fault: true,
            ..Sink::default()
        };
        let mut cursor = 0;
        assert_eq!(
            step(&mut sink, b"abc\n", &mut cursor),
            Err(DrainError::Write(()))
        );
        assert_eq!(cursor, 0);
        assert!(sink.bytes.is_empty());
    }

    #[test]
    fn pressure_has_bounded_capacity_and_no_partial_or_recursive_output() {
        let mut queue = heapless::Deque::<MsgBuffer, 4>::new();
        let mut queued = 0;
        let mut dropped = 0;
        for n in 0..100 {
            match enqueue_diagnostic(log::Level::Info, format_args!("button {n}"), |line| {
                queue.push_back(line).is_ok()
            }) {
                Enqueue::Queued => queued += 1,
                Enqueue::Full => dropped += 1,
                Enqueue::FormatError => panic!("short message"),
            }
        }
        assert_eq!((queued, dropped, queue.len()), (4, 96, 4));
        let outcome = enqueue_diagnostic(
            log::Level::Info,
            format_args!("{}", "x".repeat(512)),
            |_| panic!("must not enqueue partial line"),
        );
        assert_eq!(outcome, Enqueue::FormatError);
        for n in 0..4 {
            assert_eq!(
                queue.pop_front().unwrap().as_slice(),
                format!("INFO - button {n}\n").as_bytes()
            );
        }
        assert!(queue.is_empty());
    }

    #[test]
    fn full_fifo_and_slow_progress_both_reach_original_deadline() {
        for space in [0, 1] {
            let mut sink = Sink::default();
            let mut now = 0;
            let result = drain(
                &[b'x'; 512],
                |line, offset| {
                    sink.space = space;
                    step(&mut sink, line, offset)
                },
                || {
                    let before = now;
                    now += 1000;
                    before
                },
            );
            assert_eq!(result, Err(LineError::Timeout));
            assert!(now <= 21_000);
            assert_eq!(sink.bytes.len(), if space == 0 { 0 } else { 19 });
        }
    }

    #[test]
    fn partial_record_timeout_is_not_a_complete_frame() {
        let mut sink = Sink::default();
        let mut now = 0;
        let result = drain(
            b"{\"type\":\"ble_scan\"}\n",
            |line, offset| {
                sink.space = usize::from(sink.bytes.len() < 5);
                step(&mut sink, line, offset)
            },
            || {
                let before = now;
                now += 1000;
                before
            },
        );
        assert_eq!(result, Err(LineError::Timeout));
        assert_eq!(sink.bytes, b"{\"typ");
        assert!(!sink.bytes.contains(&b'\n'));
    }

    #[test]
    fn deadline_survives_clock_wrap_and_complete_line_is_exact() {
        let mut sink = Sink::default();
        let line = [b'x'; 512];
        let mut now = u64::MAX - 100;
        assert_eq!(
            drain(
                &line,
                |line, offset| {
                    sink.space = 7;
                    step(&mut sink, line, offset)
                },
                || {
                    let before = now;
                    now = now.wrapping_add(20);
                    before
                }
            ),
            Ok(())
        );
        assert_eq!(sink.bytes, line);
    }

    #[test]
    fn invalid_zero_or_excess_progress_fails_immediately() {
        struct Invalid(usize);
        impl Fifo for Invalid {
            type Error = ();
            fn ready(&mut self) -> bool {
                true
            }
            fn write(&mut self, _: &[u8]) -> Result<usize, ()> {
                Ok(self.0)
            }
        }
        for n in [0, 17] {
            assert_eq!(
                step(&mut Invalid(n), &[0; 16], &mut 0),
                Err(DrainError::InvalidProgress)
            );
        }
    }

    #[test]
    fn failed_line_permanently_disables_uart_without_splicing_later_frames() {
        let mut writer = Writer::default();
        let mut sink = Sink {
            space: 3,
            ..Sink::default()
        };
        let mut now = 0;
        assert_eq!(
            writer.write(
                b"{\"first\":1}\n",
                |line, cursor| step(&mut sink, line, cursor),
                || {
                    let before = now;
                    now += 1000;
                    before
                }
            ),
            WriteOutcome::Failed
        );
        let prefix = sink.bytes.clone();
        assert_eq!(
            writer.write(
                b"{\"second\":2}\n",
                |_, _| -> Result<Step, ()> { panic!("no more hardware writes") },
                || panic!("no polling")
            ),
            WriteOutcome::Disabled
        );
        assert_eq!(sink.bytes, prefix);
        assert_eq!(sink.bytes, b"{\"f");
    }
}
