//! Bounded framing/correlation around pinned generated Flipper RPC messages.
//! This module does not open ports, launch applications, or issue radio commands.
use flipper_rpc::proto::{main::Content, Main};
use prost::Message;
use std::io;

pub const MAX_FRAME: usize = 1024;
pub const MAX_READ: usize = 512;

#[derive(Default)]
pub struct Decoder {
    buffer: Vec<u8>,
    failed: bool,
}

impl Decoder {
    /// Feed at most one bounded serial read. Partial length/body bytes survive
    /// cancellation of the surrounding IO wait because decoding owns the buffer.
    pub fn feed(&mut self, bytes: &[u8]) -> io::Result<Vec<Main>> {
        if self.failed {
            return Err(invalid("Flipper RPC framing already failed"));
        }
        let result = self.consume(bytes);
        if result.is_err() {
            self.failed = true;
            self.buffer.clear();
        }
        result
    }

    fn consume(&mut self, bytes: &[u8]) -> io::Result<Vec<Main>> {
        if bytes.len() > MAX_READ || self.buffer.len() + bytes.len() > MAX_FRAME + 5 + MAX_READ {
            return Err(invalid("Flipper RPC input exceeds bounded read"));
        }
        self.buffer.extend_from_slice(bytes);
        let mut result = Vec::new();
        loop {
            let mut length = 0usize;
            let mut prefix = None;
            for (i, byte) in self.buffer.iter().take(5).enumerate() {
                if i == 4 && *byte > 15 {
                    return Err(invalid("Invalid Flipper RPC frame length"));
                }
                length |= usize::from(byte & 0x7f) << (i * 7);
                if byte & 0x80 == 0 {
                    if length == 0 || length > MAX_FRAME || (i > 0 && *byte == 0) {
                        return Err(invalid("Invalid or oversized Flipper RPC frame"));
                    }
                    prefix = Some(i + 1);
                    break;
                }
            }
            let Some(prefix) = prefix else {
                if self.buffer.len() >= 5 {
                    return Err(invalid("Unterminated Flipper RPC length"));
                }
                break;
            };
            if self.buffer.len() < prefix + length {
                break;
            }
            let message = Main::decode(&self.buffer[prefix..prefix + length])
                .map_err(|_| invalid("Invalid Flipper RPC protobuf"))?;
            self.buffer.drain(..prefix + length);
            if message.has_next || message.content.is_none() {
                return Err(invalid(
                    "Unexpected fragmented or empty Flipper RPC envelope",
                ));
            }
            result.push(message);
        }
        Ok(result)
    }
}

pub fn encode(message: &Main) -> io::Result<Vec<u8>> {
    if message.encoded_len() > MAX_FRAME || message.content.is_none() || message.has_next {
        return Err(invalid("Unsupported outgoing Flipper RPC envelope"));
    }
    Ok(message.encode_length_delimited_to_vec())
}

/// One RPC command can be pending at a time. A transport ACK never means the
/// attached radio started/stopped; those require independent bridge evidence.
#[derive(Default)]
pub struct Correlation {
    next: u32,
    pending: Option<u32>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Received {
    TransportAck { command: u32 },
    ApplicationData(Vec<u8>),
    ApplicationState(i32),
}

impl Correlation {
    pub fn awaiting_ack(&self) -> bool {
        self.pending.is_some()
    }

    pub fn request(&mut self, content: Content) -> io::Result<Main> {
        if self.pending.is_some() {
            return Err(invalid("Flipper RPC command still pending"));
        }
        // Closed allowlist excludes filesystem writes, reboot, GUI input and
        // arbitrary UART text. AppData payload is validated by the bridge codec.
        if !matches!(
            content,
            Content::AppStartRequest(_)
                | Content::AppExitRequest(_)
                | Content::AppDataExchangeRequest(_)
                | Content::StopSession(_)
        ) {
            return Err(invalid("Unsupported Flipper RPC command"));
        }
        if let Content::AppDataExchangeRequest(data) = &content {
            let message = airhound_flipper::bridge::decode(&data.data)
                .map_err(|_| invalid("Invalid outgoing bridge payload"))?;
            if !matches!(message.body, airhound_flipper::bridge::Body::Command(_)) {
                return Err(invalid("Outgoing bridge payload is not a host command"));
            }
        }
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| invalid("Flipper RPC command sequence exhausted"))?;
        self.pending = Some(self.next);
        Ok(Main {
            command_id: self.next,
            command_status: 0,
            has_next: false,
            content: Some(content),
        })
    }

    pub fn receive(&mut self, message: Main) -> io::Result<Received> {
        if message.has_next || message.command_status != 0 {
            return Err(invalid(
                "Flipper RPC reported error or unexpected continuation",
            ));
        }
        match (message.command_id, message.content) {
            (0, Some(Content::AppDataExchangeRequest(data))) if data.data.len() <= 300 => {
                Ok(Received::ApplicationData(data.data))
            }
            (0, Some(Content::AppStateResponse(state))) if (0..=1).contains(&state.state) => {
                Ok(Received::ApplicationState(state.state))
            }
            (id, Some(Content::Empty(_))) if id != 0 && self.pending == Some(id) => {
                self.pending = None;
                Ok(Received::TransportAck { command: id })
            }
            _ => Err(invalid("Unexpected Flipper RPC response identity or type")),
        }
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use flipper_rpc::proto::{self, app};

    #[test]
    fn pinned_official_python_goldens_roundtrip_in_both_directions() {
        let fixtures: &[&[u8]] = &[
            include_bytes!("../tests/fixtures/flipper-rpc/start.bin"),
            include_bytes!("../tests/fixtures/flipper-rpc/exit.bin"),
            include_bytes!("../tests/fixtures/flipper-rpc/data.bin"),
            include_bytes!("../tests/fixtures/flipper-rpc/event.bin"),
            include_bytes!("../tests/fixtures/flipper-rpc/ack.bin"),
            include_bytes!("../tests/fixtures/flipper-rpc/busy.bin"),
            include_bytes!("../tests/fixtures/flipper-rpc/started.bin"),
            include_bytes!("../tests/fixtures/flipper-rpc/stop_session.bin"),
            include_bytes!("../tests/fixtures/flipper-rpc/ping.bin"),
            include_bytes!("../tests/fixtures/flipper-rpc/pong.bin"),
        ];
        for bytes in fixtures {
            for width in 1..=bytes.len() {
                let mut decoder = Decoder::default();
                let mut decoded = vec![];
                for part in bytes.chunks(width) {
                    decoded.extend(decoder.feed(part).unwrap());
                }
                assert_eq!(decoded.len(), 1);
                assert_eq!(encode(&decoded[0]).unwrap(), *bytes);
            }
        }
        let data = Main {
            command_id: 3,
            content: Some(Content::AppDataExchangeRequest(app::DataExchangeRequest {
                data: (0..=255).collect(),
            })),
            ..Default::default()
        };
        assert_eq!(encode(&data).unwrap(), fixtures[2]);
        let start = Main {
            command_id: 1,
            content: Some(Content::AppStartRequest(app::StartRequest {
                name: "/ext/apps/GPIO/airhound_bridge.fap".into(),
                args: "RPC".into(),
            })),
            ..Default::default()
        };
        assert_eq!(encode(&start).unwrap(), fixtures[0]);
    }

    #[test]
    fn framing_rejects_oversize_noncanonical_and_truncated_varints_without_growth() {
        for data in [
            &[0u8][..],
            &[0x80, 0],
            &[0xff; 5],
            &[0x81, 0x08],
            &[0xff, 0xff, 0xff, 0xff, 0x10],
        ] {
            let mut decoder = Decoder::default();
            assert!(decoder.feed(data).is_err());
            assert!(decoder.feed(&[2, 0x22, 0]).is_err());
            assert!(decoder.buffer.is_empty());
        }
        assert!(Decoder::default().feed(&vec![0; MAX_READ + 1]).is_err());
    }

    #[test]
    fn unsolicited_events_do_not_ack_commands_and_stale_ids_are_rejected() {
        let mut c = Correlation::default();
        let request = c
            .request(Content::AppExitRequest(app::AppExitRequest {}))
            .unwrap();
        assert!(c
            .request(Content::StopSession(proto::StopSession {}))
            .is_err());
        let event = Main {
            content: Some(Content::AppDataExchangeRequest(app::DataExchangeRequest {
                data: vec![1],
            })),
            ..Default::default()
        };
        assert_eq!(
            c.receive(event).unwrap(),
            Received::ApplicationData(vec![1])
        );
        assert_eq!(c.pending, Some(request.command_id));
        let mut ack = Main {
            command_id: request.command_id + 1,
            content: Some(Content::Empty(proto::Empty {})),
            ..Default::default()
        };
        assert!(c.receive(ack.clone()).is_err());
        ack.command_id = request.command_id;
        assert_eq!(
            c.receive(ack.clone()).unwrap(),
            Received::TransportAck {
                command: request.command_id
            }
        );
        assert!(c.receive(ack).is_err());
    }
}
