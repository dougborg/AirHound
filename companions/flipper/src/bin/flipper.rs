//! Synthetic parser demonstration only: no UART, GPIO, RF or command transport.
#![no_std]
#![no_main]
extern crate flipperzero_alloc;
extern crate flipperzero_rt;
use airhound_flipper::Receiver;
use core::{ffi::CStr, fmt::Write};
use flipperzero_rt::{entry, manifest};
manifest!(
    name = "AirHound parser demo",
    stack_size = 8192,
    app_version = 1
);
entry!(main);
fn main(_args: Option<&CStr>) -> i32 {
    let mut receiver = Receiver::new();
    receiver.begin_session();
    let fixture = b"{\"type\":\"wifi\",\"mac\":\"02:00:00:00:00:01\",\"ssid\":\"demo\",\"rssi\":-50,\"ch\":1,\"frame\":\"beacon\",\"match\":[],\"ts\":1}\n";
    for &b in fixture {
        let _ = receiver.feed(b);
    }
    receiver.disconnect();
    let mut text = heapless::String::<160>::new();
    let _ = write!(
        text,
        "SYNTHETIC DEMO\nAP sightings: {}\nSignature sightings: {}\nNo UART; disconnected",
        receiver.counts.access_point_observations, receiver.counts.signature_observations
    );
    flipperzero::dialogs::alert(text.as_str());
    0
}
