//! Cooperative scheduling for the two periodic health records.
//!
//! A preceding optional diagnostic and these records must not form a three-line
//! burst in one executor poll. Formatting/enqueueing stays synchronous, so no
//! 512-byte message buffer is stored across an await. A yield offers the consumer
//! a turn; it does not reserve queue space or promise lossless delivery.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Status,
    StreamStats,
}

pub async fn publish_pair(mut publish: impl FnMut(Record)) {
    embassy_futures::yield_now().await;
    publish(Record::Status);
    embassy_futures::yield_now().await;
    publish(Record::StreamStats);
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{
        cell::Cell,
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    use embassy_sync::{blocking_mutex::raw::NoopRawMutex, channel::Channel};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Item {
        Observation,
        ScanLossDiagnostic,
        Health(Record),
    }
    type Output = Channel<NoopRawMutex, Item, 4>;

    fn busy_output() -> Output {
        let output = Output::new();
        for _ in 0..3 {
            output.try_send(Item::Observation).unwrap();
        }
        output.try_send(Item::ScanLossDiagnostic).unwrap();
        output
    }

    #[test]
    fn prior_same_poll_burst_rejects_both_health_records() {
        let output = busy_output();
        let mut drops = 0;
        // The old status_task did this immediately after ScanLossDiagnostic,
        // without returning to the executor between either publication.
        for record in [Record::Status, Record::StreamStats] {
            if output.try_send(Item::Health(record)).is_err() {
                drops += 1;
            }
        }
        assert_eq!(drops, 2);
        assert_eq!(output.len(), 4);
    }

    #[test]
    fn consumer_turn_between_publications_prevents_the_same_burst() {
        let output = busy_output();
        let drops = Cell::new(0);
        let publisher = publish_pair(|record| {
            if output.try_send(Item::Health(record)).is_err() {
                drops.set(drops.get() + 1);
            }
        });
        // The helper stores scheduling state and references, not a fifth record.
        assert!(core::mem::size_of_val(&publisher) < 64);
        let mut publisher = pin!(publisher);
        let mut cx = Context::from_waker(Waker::noop());

        assert_eq!(publisher.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(output.try_receive(), Ok(Item::Observation));
        assert_eq!(publisher.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(output.try_receive(), Ok(Item::Observation));
        assert_eq!(publisher.as_mut().poll(&mut cx), Poll::Ready(()));
        assert_eq!(drops.get(), 0);
        assert_eq!(output.try_receive(), Ok(Item::Observation));
        assert_eq!(output.try_receive(), Ok(Item::ScanLossDiagnostic));
        assert_eq!(output.try_receive(), Ok(Item::Health(Record::Status)));
        assert_eq!(output.try_receive(), Ok(Item::Health(Record::StreamStats)));
        assert!(output.is_empty());
    }

    #[test]
    fn competing_producers_can_still_fill_space_and_losses_remain_visible() {
        let output = busy_output();
        let drops = Cell::new(0);
        let attempted = Cell::new(0);
        let publisher = publish_pair(|record| {
            attempted.set(attempted.get() + 1);
            if output.try_send(Item::Health(record)).is_err() {
                drops.set(drops.get() + 1);
            }
        });
        let mut publisher = pin!(publisher);
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(publisher.as_mut().poll(&mut cx), Poll::Pending);
        // A busy observation producer takes each newly available slot first.
        output.try_receive().unwrap();
        output.try_send(Item::Observation).unwrap();
        assert_eq!(publisher.as_mut().poll(&mut cx), Poll::Pending);
        output.try_receive().unwrap();
        output.try_send(Item::Observation).unwrap();
        assert_eq!(publisher.as_mut().poll(&mut cx), Poll::Ready(()));
        assert_eq!((attempted.get(), drops.get()), (2, 2));
    }
}
