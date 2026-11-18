#![no_main]
use airhound::{comm, protocol::MAX_MSG_LEN};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = comm::parse_command(data);
    let mut reader = comm::LineReader::new();
    let mut length = 0;
    for byte in data.iter().copied().chain([b'\n']) {
        let line = reader.feed(byte);
        if byte == b'\n' || byte == b'\r' {
            if length > MAX_MSG_LEN {
                assert!(line.is_none());
            }
            if let Some(line) = line {
                assert!(line.len() <= MAX_MSG_LEN);
                let _ = comm::parse_command(line);
            }
            length = 0;
        } else {
            length += 1;
        }
    }
    // Every stream must recover at the next line boundary.
    for &byte in b"{\"cmd\":\"status\"}" {
        assert!(reader.feed(byte).is_none());
    }
    assert!(matches!(
        comm::parse_command(reader.feed(b'\n').unwrap()),
        Some(airhound::protocol::HostCommand::GetStatus)
    ));
});
