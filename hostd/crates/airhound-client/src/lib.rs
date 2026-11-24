//! Typed control calls shared by CLI, TUI and MCP. No receiver or policy implementation.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
use airhound_api::{ApiError, Command, Hello, Request, Response, MAX_FRAME_BYTES, VERSION};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::{io, path::Path, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;

/// A 4-byte big-endian length followed by UTF-8 JSON; reject before allocating.
/// Generic IO keeps framing usable by a future authenticated transport adapter.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Vec<u8>> {
    let length = reader.read_u32().await? as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid control frame length",
        ));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes).await?;
    Ok(bytes)
}
pub async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(
    writer: &mut W,
    value: &T,
) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "control frame exceeds limit",
        ));
    }
    writer.write_u32(bytes.len() as u32).await?;
    writer.write_all(&bytes).await?;
    writer.flush().await
}

#[derive(Debug)]
pub enum ClientError {
    Io(io::Error),
    Protocol(String),
    Remote(ApiError),
    Timeout,
    Closed,
}
impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => e.fmt(f),
            Self::Protocol(s) => write!(f, "control protocol: {s}"),
            Self::Remote(e) => e.fmt(f),
            Self::Timeout => f.write_str("control request timed out"),
            Self::Closed => f.write_str("control connection closed; reconnect before retrying"),
        }
    }
}
impl std::error::Error for ClientError {}
impl From<io::Error> for ClientError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// One outstanding request per client. A timeout/protocol failure poisons the
/// stream, preventing a late response being associated with a subsequent call.
pub struct Client<S = UnixStream> {
    stream: Option<S>,
    next_id: u64,
    timeout: Duration,
    pub hello: Hello,
}
impl Client<UnixStream> {
    pub async fn connect(path: &Path, timeout: Duration) -> Result<Self, ClientError> {
        let stream = tokio::time::timeout(timeout, UnixStream::connect(path))
            .await
            .map_err(|_| ClientError::Timeout)??;
        // Authenticate the local peer before sending configuration bytes. A future
        // remote transport must provide its own authentication before handshake.
        if stream.peer_cred()?.uid() != unsafe { libc::geteuid() } {
            return Err(ClientError::Protocol(
                "backend has a different effective user".into(),
            ));
        }
        Self::handshake(stream, timeout).await
    }
}
impl<S: AsyncRead + AsyncWrite + Unpin> Client<S> {
    pub async fn handshake(stream: S, timeout: Duration) -> Result<Self, ClientError> {
        let mut client = Self {
            stream: Some(stream),
            next_id: 1,
            timeout,
            hello: Hello {
                version: VERSION,
                instance_id: String::new(),
                build_version: String::new(),
                max_frame_bytes: MAX_FRAME_BYTES,
                methods: vec![],
                mode: String::new(),
            },
        };
        client.hello = client.call(Command::Hello {}).await?;
        if client.hello.version != VERSION
            || client.hello.max_frame_bytes != MAX_FRAME_BYTES
            || client.hello.instance_id.is_empty()
        {
            return Err(ClientError::Protocol("incompatible hello".into()));
        }
        Ok(client)
    }
    pub async fn identify_device(&mut self, config: Value) -> Result<Value, ClientError> {
        self.call(Command::DevicesIdentify { config }).await
    }
    pub async fn list_profiles(&mut self) -> Result<Value, ClientError> {
        self.call(Command::ProfilesList {}).await
    }
    pub async fn save_profile(
        &mut self,
        id: String,
        name: String,
        config: Value,
        replace: bool,
    ) -> Result<Value, ClientError> {
        self.call(Command::ProfilesSave {
            id,
            name,
            config,
            replace,
        })
        .await
    }
    pub async fn load_profile(&mut self, id: String) -> Result<Value, ClientError> {
        self.call(Command::ProfilesLoad { id }).await
    }
    pub async fn delete_profile(&mut self, id: String) -> Result<Value, ClientError> {
        self.call(Command::ProfilesDelete { id }).await
    }
    pub async fn status(&mut self) -> Result<Value, ClientError> {
        self.call(Command::Status {}).await
    }
    pub async fn export_recordings(&mut self, destination: String) -> Result<Value, ClientError> {
        self.call(Command::RecordingExport { destination }).await
    }
    pub async fn capture(
        &mut self,
        action: airhound_api::CaptureAction,
    ) -> Result<Value, ClientError> {
        self.call(Command::Capture { action }).await
    }
    pub async fn recordings(
        &mut self,
        before: Option<String>,
        limit: u16,
    ) -> Result<Value, ClientError> {
        self.call(Command::RecordingsList { before, limit }).await
    }
    pub async fn recording_records(
        &mut self,
        run_id: String,
        after: i64,
        limit: u16,
    ) -> Result<Value, ClientError> {
        self.call(Command::RecordingRecords {
            run_id,
            after,
            limit,
        })
        .await
    }
    pub async fn devices(&mut self, all: bool) -> Result<Value, ClientError> {
        self.call(Command::DevicesList { all }).await
    }
    pub async fn validate(&mut self, config: Value) -> Result<Value, ClientError> {
        self.call(Command::SessionValidate { config }).await
    }

    async fn call<T: DeserializeOwned>(&mut self, command: Command) -> Result<T, ClientError> {
        // Take ownership across await: dropping/cancelling this future closes the connection.
        let mut stream = self.stream.take().ok_or(ClientError::Closed)?;
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| ClientError::Protocol("request IDs exhausted".into()))?;
        let operation = async {
            write_frame(
                &mut stream,
                &Request {
                    version: VERSION,
                    request_id: id,
                    command,
                },
            )
            .await?;
            let bytes = read_frame(&mut stream).await?;
            let response: Response =
                serde_json::from_slice(&bytes).map_err(|e| ClientError::Protocol(e.to_string()))?;
            if response.version != VERSION || response.request_id != id {
                return Err(ClientError::Protocol(
                    "response version or request ID mismatch".into(),
                ));
            }
            match (response.result, response.error) {
                (Some(value), None) => {
                    serde_json::from_value(value).map_err(|e| ClientError::Protocol(e.to_string()))
                }
                (None, Some(error)) => Err(ClientError::Remote(error)),
                _ => Err(ClientError::Protocol(
                    "response must contain exactly one result or error".into(),
                )),
            }
        };
        let result = tokio::time::timeout(self.timeout, operation)
            .await
            .map_err(|_| ClientError::Timeout)?;
        if result.is_ok() || matches!(result, Err(ClientError::Remote(_))) {
            self.stream = Some(stream);
        }
        result
    }
}

#[cfg(test)]
mod test_clock;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use airhound_api::ErrorCode;
    use tokio::io::duplex;

    fn hello() -> Value {
        serde_json::to_value(Hello {
            version: VERSION,
            instance_id: "test".into(),
            build_version: "test".into(),
            max_frame_bytes: MAX_FRAME_BYTES,
            methods: vec!["hello".into()],
            mode: "passive_control".into(),
        })
        .unwrap()
    }
    #[tokio::test(start_paused = true)]
    async fn framing_rejects_lengths_and_truncated_payloads() {
        for bytes in [
            0u32.to_be_bytes().to_vec(),
            ((MAX_FRAME_BYTES + 1) as u32).to_be_bytes().to_vec(),
            vec![0, 0, 0, 4, 1],
        ] {
            let mut input = bytes.as_slice();
            assert!(read_frame(&mut input).await.is_err());
        }
        let mut output = Vec::new();
        assert!(write_frame(&mut output, &"x".repeat(MAX_FRAME_BYTES))
            .await
            .is_err());
        assert!(output.is_empty());
    }
    #[tokio::test(start_paused = true)]
    async fn fragmented_frame_and_remote_error_preserve_connection() {
        let time = super::test_clock::ManualTime::hold().await;
        let (client, mut server) = duplex(64);
        let server_task = tokio::spawn(async move {
            for id in 1..=3 {
                let request: Request =
                    serde_json::from_slice(&read_frame(&mut server).await.unwrap()).unwrap();
                assert_eq!(request.request_id, id);
                let reply = if id == 1 {
                    Response::success(id, hello())
                } else if id == 2 {
                    Response::failure(id, ApiError::new(ErrorCode::InvalidSession, "bad plan"))
                } else {
                    Response::success(id, serde_json::json!({"mode":"passive_control"}))
                };
                write_frame(&mut server, &reply).await.unwrap();
            }
        });
        let mut client = Client::handshake(client, Duration::from_secs(1))
            .await
            .unwrap();
        assert!(matches!(
            client.validate(Value::Null).await,
            Err(ClientError::Remote(_))
        ));
        assert_eq!(client.status().await.unwrap()["mode"], "passive_control");
        server_task.await.unwrap();
        time.finish().await;
    }
    #[tokio::test(start_paused = true)]
    async fn mismatched_response_poisons_connection() {
        let time = super::test_clock::ManualTime::hold().await;
        let (client, mut server) = duplex(1024);
        let peer = tokio::spawn(async move {
            read_frame(&mut server).await.unwrap();
            write_frame(&mut server, &Response::success(1, hello()))
                .await
                .unwrap();
            read_frame(&mut server).await.unwrap();
            write_frame(&mut server, &Response::success(99, Value::Null))
                .await
                .unwrap();
            assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
        });
        let mut client = Client::handshake(client, Duration::from_secs(1))
            .await
            .unwrap();
        assert!(
            matches!(client.status().await, Err(ClientError::Protocol(message)) if message == "response version or request ID mismatch")
        );
        assert!(matches!(client.status().await, Err(ClientError::Closed)));
        peer.await.unwrap();
        time.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn request_timeout_boundary_poisons_connection_and_closes_peer() {
        let time = super::test_clock::ManualTime::hold().await;
        let (client, mut server) = duplex(1024);
        let (seen, received) = tokio::sync::oneshot::channel();
        let peer = tokio::spawn(async move {
            read_frame(&mut server).await.unwrap();
            write_frame(&mut server, &Response::success(1, hello()))
                .await
                .unwrap();
            let request: Request =
                serde_json::from_slice(&read_frame(&mut server).await.unwrap()).unwrap();
            assert!(matches!(request.command, Command::Status {}));
            seen.send(()).unwrap();
            assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
        });
        let mut client = Client::handshake(client, Duration::from_secs(1))
            .await
            .unwrap();
        {
            let call = client.status();
            tokio::pin!(call);
            tokio::select! { result = &mut call => panic!("early result: {result:?}"), _ = received => {} }
            tokio::time::advance(Duration::from_millis(999)).await;
            assert!(std::future::Future::poll(
                call.as_mut(),
                &mut std::task::Context::from_waker(std::task::Waker::noop())
            )
            .is_pending());
            tokio::time::advance(Duration::from_millis(1)).await;
            assert!(matches!(call.await, Err(ClientError::Timeout)));
        }
        assert!(matches!(client.status().await, Err(ClientError::Closed)));
        peer.await.unwrap();
        time.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn handshake_timeout_uses_virtual_deadline_and_closes_peer() {
        let time = super::test_clock::ManualTime::hold().await;
        let (client, mut server) = duplex(1024);
        let (seen, received) = tokio::sync::oneshot::channel();
        let peer = tokio::spawn(async move {
            let request: Request =
                serde_json::from_slice(&read_frame(&mut server).await.unwrap()).unwrap();
            assert!(matches!(request.command, Command::Hello {}));
            seen.send(()).unwrap();
            assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
        });
        let call = Client::handshake(client, Duration::from_secs(1));
        tokio::pin!(call);
        tokio::select! { _ = &mut call => panic!("unexpected handshake completion"), _ = received => {} }
        tokio::time::advance(Duration::from_millis(999)).await;
        assert!(std::future::Future::poll(
            call.as_mut(),
            &mut std::task::Context::from_waker(std::task::Waker::noop())
        )
        .is_pending());
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(matches!(call.await, Err(ClientError::Timeout)));
        peer.await.unwrap();
        time.finish().await;
    }
    #[tokio::test(start_paused = true)]
    async fn cancelled_call_closes_stream_instead_of_accepting_late_response() {
        let time = super::test_clock::ManualTime::hold().await;
        let (client, mut server) = duplex(1024);
        let (seen, started) = tokio::sync::oneshot::channel();
        let peer = tokio::spawn(async move {
            read_frame(&mut server).await.unwrap();
            write_frame(&mut server, &Response::success(1, hello()))
                .await
                .unwrap();
            read_frame(&mut server).await.unwrap();
            seen.send(()).unwrap();
            // The dropped request future must close this connection.
            let mut byte = [0];
            assert_eq!(server.read(&mut byte).await.unwrap(), 0);
        });
        let mut client = Client::handshake(client, Duration::from_secs(1))
            .await
            .unwrap();
        {
            let call = client.status();
            tokio::pin!(call);
            tokio::select! { _ = &mut call => panic!("unexpected response"), _ = started => {} }
        }
        assert!(matches!(client.status().await, Err(ClientError::Closed)));
        peer.await.unwrap();
        time.finish().await;
    }
}
