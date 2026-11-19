//! Best-effort private-API address resolution on one bounded worker thread.
//! The UUID remains the primary local identity; returned address type is unknown.
use std::sync::mpsc::{sync_channel, SyncSender};
use tokio::sync::oneshot;

type Request = ([u8; 16], oneshot::Sender<Option<[u8; 6]>>);

pub struct AddressResolver {
    requests: Option<SyncSender<Request>>,
}

impl AddressResolver {
    pub fn new() -> Self {
        let mut unavailable = false;
        Self::with_lookup(move |id| {
            if unavailable {
                return None;
            }
            match corebluetooth_address::peripheral_address(id) {
                Ok(address) => address,
                Err(
                    corebluetooth_address::Error::SelectorMissing
                    | corebluetooth_address::Error::Unsupported,
                ) => {
                    log::warn!("BLE address lookup unavailable; retaining peripheral UUIDs");
                    unavailable = true;
                    None
                }
                Err(error) => {
                    log::debug!("BLE address lookup failed: {error}");
                    None
                }
            }
        })
    }

    fn with_lookup(mut lookup: impl FnMut([u8; 16]) -> Option<[u8; 6]> + Send + 'static) -> Self {
        let (send, receive) = sync_channel::<Request>(1);
        let worker = std::thread::Builder::new()
            .name("airhound-ble-address".into())
            .spawn(move || {
                while let Ok((id, reply)) = receive.recv() {
                    if reply.is_closed() {
                        continue;
                    }
                    // Rust worker threads do not install an Objective-C pool.
                    // Drain temporaries from every native lookup, even when its
                    // async caller has timed out or paused in the meantime.
                    let result = objc2::rc::autoreleasepool(|_| lookup(id));
                    let _ = reply.send(result);
                }
            });
        if let Err(error) = worker {
            log::warn!("Could not start BLE address lookup: {error}; retaining peripheral UUIDs");
            return Self { requests: None };
        }
        Self {
            requests: Some(send),
        }
    }

    pub async fn lookup(&self, id: &str) -> Option<[u8; 6]> {
        let id = uuid::Uuid::parse_str(id).ok()?;
        let (send, receive) = oneshot::channel();
        self.requests
            .as_ref()?
            .try_send((*id.as_bytes(), send))
            .ok()?;
        // Never queue unbounded work or wait for the OS's potentially 10s timeout.
        let result = tokio::time::timeout(std::time::Duration::from_millis(25), receive)
            .await
            .ok()?
            .ok()??;
        (result != [0; 6] && result != [0xff; 6]).then_some(result)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    const ID: &str = "00112233-4455-6677-8899-aabbccddeeff";
    #[tokio::test]
    async fn lookup_preserves_byte_order_and_rejects_missing_or_invalid_addresses() {
        let resolver = AddressResolver::with_lookup(|id| {
            assert_eq!(id[0..4], [0, 0x11, 0x22, 0x33]);
            Some([0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc])
        });
        assert_eq!(
            resolver.lookup(ID).await,
            Some([0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc])
        );
        assert_eq!(resolver.lookup("not a UUID").await, None);
        for address in [None, Some([0; 6]), Some([0xff; 6])] {
            assert_eq!(
                AddressResolver::with_lookup(move |_| address)
                    .lookup(ID)
                    .await,
                None
            );
        }
        assert_eq!(AddressResolver { requests: None }.lookup(ID).await, None);
    }
    #[tokio::test]
    async fn blocked_or_panicking_native_lookup_falls_back_without_blocking_shutdown() {
        let (release, blocked) = std::sync::mpsc::channel();
        let resolver = AddressResolver::with_lookup(move |_| {
            let _ = blocked.recv();
            Some([1; 6])
        });
        assert_eq!(resolver.lookup(ID).await, None);
        drop(resolver);
        release.send(()).unwrap();
        let resolver = AddressResolver::with_lookup(|_| panic!("test native failure"));
        assert_eq!(resolver.lookup(ID).await, None);
        assert_eq!(resolver.lookup(ID).await, None);
    }
    #[tokio::test]
    async fn native_temporaries_are_released_between_worker_requests() {
        use objc2::rc::{Retained, Weak};
        use objc2_foundation::NSObject;
        use std::cell::RefCell;
        thread_local! {
            static PREVIOUS: RefCell<Option<Weak<NSObject>>> = const { RefCell::new(None) };
        }
        let resolver = AddressResolver::with_lookup(|_| {
            PREVIOUS.with(|previous| {
                if let Some(weak) = previous.borrow().as_ref() {
                    assert!(
                        weak.load().is_none(),
                        "previous lookup's autoreleased object leaked"
                    );
                }
                let object = NSObject::new();
                *previous.borrow_mut() = Some(Weak::from_retained(&object));
                // Models a native API returning an autoreleased temporary.
                let _ = Retained::autorelease_ptr(object);
            });
            Some([1; 6])
        });
        for _ in 0..3 {
            assert_eq!(resolver.lookup(ID).await, Some([1; 6]));
        }
    }
}
