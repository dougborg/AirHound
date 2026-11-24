//! Yield before each M5 HCI packet read, including immediately-ready backlogs.
//!
//! The radio callback stays nonblocking. This gives the scan-event consumer and
//! UART tasks a cooperative scheduling opportunity between HCI packets; it does
//! not promise fairness inside a multi-report packet or zero loss at any load.
use bt_hci::{transport::Transport, ControllerToHostPacket, HostToControllerPacket};

pub struct CooperativeTransport<T> {
    inner: T,
}

impl<T> CooperativeTransport<T> {
    pub const fn new(inner: T) -> Self {
        Self { inner }
    }
}

impl<T: embedded_io::ErrorType> embedded_io::ErrorType for CooperativeTransport<T> {
    type Error = T::Error;
}

impl<T: Transport> Transport for CooperativeTransport<T> {
    async fn read<'a>(&self, rx: &'a mut [u8]) -> Result<ControllerToHostPacket<'a>, Self::Error> {
        // Yield before consuming anything: cancellation here leaves the packet
        // in the underlying transport. No extra packet storage or timer needed.
        embassy_futures::yield_now().await;
        self.inner.read(rx).await
    }

    async fn write<P: HostToControllerPacket>(&self, packet: &P) -> Result<(), Self::Error> {
        self.inner.write(packet).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::{BleAdvParser, ScanEvent};
    use bt_hci::{cmd::le::LeSetScanEnable, param::LeAdvReports, FromHciBytes};
    use core::{
        cell::{Cell, RefCell},
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    use embassy_sync::{blocking_mutex::raw::NoopRawMutex, channel::Channel};

    struct ReadyTransport {
        reads: Cell<usize>,
        fail: Cell<bool>,
        writes: RefCell<std::vec::Vec<u8>>,
    }
    impl ReadyTransport {
        fn new() -> Self {
            Self {
                reads: Cell::new(0),
                fail: Cell::new(false),
                writes: RefCell::new(std::vec::Vec::new()),
            }
        }
    }
    impl embedded_io::ErrorType for ReadyTransport {
        type Error = embedded_io::ErrorKind;
    }
    impl Transport for ReadyTransport {
        async fn read<'a>(
            &self,
            rx: &'a mut [u8],
        ) -> Result<ControllerToHostPacket<'a>, Self::Error> {
            if self.fail.get() {
                return Err(embedded_io::ErrorKind::Other);
            }
            let index = self.reads.get();
            self.reads.set(index + 1);
            // Actual HCI LE advertising event, one empty-data report. The last
            // address octet distinguishes order; these are synthetic fixtures.
            rx[..15].copy_from_slice(&[
                4,
                0x3e,
                12,
                2,
                1,
                0,
                0,
                2,
                0,
                0,
                0,
                0,
                index as u8,
                0,
                196,
            ]);
            Ok(ControllerToHostPacket::from_hci_bytes_complete(&rx[..15]).unwrap())
        }
        async fn write<P: HostToControllerPacket>(&self, packet: &P) -> Result<(), Self::Error> {
            if self.fail.get() {
                return Err(embedded_io::ErrorKind::Other);
            }
            let mut bytes = [0u8; 64];
            let mut rest = &mut bytes[..];
            packet.write_hci(&mut rest).unwrap();
            let n = 64 - rest.len();
            self.writes.borrow_mut().extend_from_slice(&bytes[..n]);
            Ok(())
        }
    }

    fn poll<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
        future.poll(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn every_ready_read_yields_before_consumption_and_preserves_packet_order() {
        let transport = CooperativeTransport::new(ReadyTransport::new());
        for index in 0..32 {
            let mut bytes = [0u8; 259];
            let mut read = pin!(transport.read(&mut bytes));
            assert!(poll(read.as_mut()).is_pending());
            assert_eq!(transport.inner.reads.get(), index);
            assert!(poll(read.as_mut()).is_ready());
            assert_eq!(transport.inner.reads.get(), index + 1);
            // Packet order is also checked by the consumer scheduling test.
        }
    }

    #[test]
    fn cancellation_at_boundary_leaves_packet_unread_and_errors_are_unchanged() {
        let transport = CooperativeTransport::new(ReadyTransport::new());
        let mut bytes = [0; 259];
        {
            let mut read = pin!(transport.read(&mut bytes));
            assert!(poll(read.as_mut()).is_pending());
        }
        assert_eq!(transport.inner.reads.get(), 0);
        transport.inner.fail.set(true);
        let mut read = pin!(transport.read(&mut bytes));
        assert!(poll(read.as_mut()).is_pending());
        assert!(matches!(
            poll(read.as_mut()),
            Poll::Ready(Err(embedded_io::ErrorKind::Other))
        ));
        assert_eq!(transport.inner.reads.get(), 0);
    }

    #[test]
    fn controller_write_is_immediate_and_byte_identical_including_errors() {
        let transport = CooperativeTransport::new(ReadyTransport::new());
        let command = LeSetScanEnable::new(true, false);
        let mut write = pin!(transport.write(&command));
        assert!(matches!(poll(write.as_mut()), Poll::Ready(Ok(()))));
        assert_eq!(&*transport.inner.writes.borrow(), &[0x0c, 0x20, 2, 1, 0]);
        transport.inner.fail.set(true);
        let mut write = pin!(transport.write(&command));
        assert!(matches!(
            poll(write.as_mut()),
            Poll::Ready(Err(embedded_io::ErrorKind::Other))
        ));
    }

    async fn produce<T: Transport>(
        transport: &T,
        channel: &Channel<NoopRawMutex, ScanEvent, 20>,
        dropped: &Cell<usize>,
    ) {
        for _ in 0..41 {
            let mut rx = [0u8; 259];
            assert!(transport.read(&mut rx).await.is_ok());
            // Decode real reports using the same parser and nonblocking channel
            // admission as the firmware callback. No sleep or fake async I/O.
            let reports = LeAdvReports::from_hci_bytes_complete(&rx[4..15]).unwrap();
            for report in reports.iter() {
                let report = report.unwrap();
                let addr: &[u8; 6] = report.addr.raw().try_into().unwrap();
                let event = BleAdvParser::parse(addr, report.rssi, report.data);
                if channel.try_send(ScanEvent::Ble(event)).is_err() {
                    dropped.set(dropped.get() + 1);
                }
            }
        }
    }

    #[test]
    fn ready_backlog_overflows_old_path_but_cooperative_reads_allow_consumer_progress() {
        let raw = ReadyTransport::new();
        let channel = Channel::<NoopRawMutex, ScanEvent, 20>::new();
        let dropped = Cell::new(0);
        let mut old = pin!(produce(&raw, &channel, &dropped));
        assert!(poll(old.as_mut()).is_ready());
        assert_eq!(dropped.get(), 21);
        while channel.try_receive().is_ok() {}
        let transport = CooperativeTransport::new(ReadyTransport::new());
        dropped.set(0);
        let mut current = pin!(produce(&transport, &channel, &dropped));
        let mut seen = std::vec::Vec::new();
        let mut finished = false;
        for _ in 0..43 {
            let done = poll(current.as_mut()).is_ready();
            if let Ok(ScanEvent::Ble(event)) = channel.try_receive() {
                seen.push(event.mac[5]);
            }
            if done {
                finished = true;
                break;
            }
        }
        assert!(finished);
        assert_eq!(dropped.get(), 0);
        assert_eq!(seen, (0u8..41).collect::<std::vec::Vec<_>>());
    }
    #[test]
    fn single_packet_report_burst_remains_a_documented_capacity_limit() {
        // Valid legacy event: 25 minimum-size reports fit the HCI 255-byte
        // event payload. No transport wrapper can yield inside this callback.
        let mut raw = std::vec![25u8];
        for n in 0..25u8 {
            raw.extend_from_slice(&[0, 0, 2, 0, 0, 0, 0, n, 0, 196]);
        }
        let reports = LeAdvReports::from_hci_bytes_complete(&raw).unwrap();
        let queue = Channel::<NoopRawMutex, ScanEvent, 20>::new();
        let mut lost = 0;
        for report in reports.iter() {
            let report = report.unwrap();
            let addr: &[u8; 6] = report.addr.raw().try_into().unwrap();
            let event = BleAdvParser::parse(addr, report.rssi, report.data);
            if queue.try_send(ScanEvent::Ble(event)).is_err() {
                lost += 1;
            }
        }
        assert_eq!(lost, 5);
    }
}
