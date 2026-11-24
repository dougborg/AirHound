//! MCP presentation only: every query goes through the shared AirHound client.
use airhound_client::{Client, ClientError};
use rmcp::{
    handler::server::wrapper::Parameters, model::CallToolResult, schemars, tool, tool_handler,
    tool_router, ServerHandler,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Semaphore;

/// Allow a bounded session document plus MCP envelope overhead.
pub const MAX_MCP_LINE_BYTES: usize = 128 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_ERROR_CHARS: usize = 2048;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionInput {
    /// Full session JSON object; paths are resolved by the backend, not this client.
    pub config: serde_json::Map<String, Value>,
}

#[derive(Clone)]
pub struct AirHoundMcp {
    socket: PathBuf,
    active: Arc<Semaphore>,
}

impl AirHoundMcp {
    pub fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            active: Arc::new(Semaphore::new(1)),
        }
    }

    async fn query(&self, query: Query) -> CallToolResult {
        // Never accumulate an unbounded queue of backend operations. Each call
        // owns its connection, so cancellation cannot desynchronize a later call.
        let Ok(_permit) = self.active.try_acquire() else {
            return local_error(
                "busy",
                "Another backend request is in progress; retry later.",
            );
        };
        let result = tokio::time::timeout(Duration::from_secs(25), async {
            let mut client = Client::connect(&self.socket, REQUEST_TIMEOUT).await?;
            match query {
                Query::Status => client.status().await,
                Query::Devices => client.devices(false).await,
                Query::Validate(config) => client.validate(config).await,
            }
        })
        .await
        .unwrap_or(Err(ClientError::Timeout));
        match result {
            Ok(value) => CallToolResult::structured(value),
            Err(ClientError::Remote(error)) => CallToolResult::structured_error(json!({
                "error": {"code":error.code,"message":bounded(&error.message),"retryable":error.retryable}
            })),
            Err(error) => local_error("backend_unavailable", &error.to_string()),
        }
    }
}

enum Query {
    Status,
    Devices,
    Validate(Value),
}

fn bounded(text: &str) -> String {
    text.chars().take(MAX_ERROR_CHARS).collect()
}
fn local_error(code: &str, message: &str) -> CallToolResult {
    CallToolResult::structured_error(json!({"error":{"code":code,"message":bounded(message)}}))
}

#[tool_router]
impl AirHoundMcp {
    #[tool(
        description = "Read the configured AirHound backend's status. Passive control availability is not an active scan or proof of radio readiness.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn backend_status(&self) -> CallToolResult {
        self.query(Query::Status).await
    }

    #[tool(
        description = "List candidate devices using the backend's passive inventory. Preserve reported uncertainty: presence does not establish permissions, capture support, tuning, or readiness. Does not open receivers.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn list_devices(&self) -> CallToolResult {
        self.query(Query::Devices).await
    }

    #[tool(
        description = "Validate a session JSON object through the backend's shared resolver and local resource metadata checks. Does not start capture. Valid configuration does not establish radio readiness; relative paths use the backend working directory.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn validate_session(
        &self,
        Parameters(input): Parameters<SessionInput>,
    ) -> CallToolResult {
        let config = Value::Object(input.config);
        if serde_json::to_vec(&config)
            .map_or(true, |bytes| bytes.len() > airhound_api::MAX_CONFIG_BYTES)
        {
            return local_error(
                "input_too_large",
                "Session JSON exceeds the backend's 64 KiB limit.",
            );
        }
        self.query(Query::Validate(config)).await
    }
}

#[tool_handler(
    name = "airhound",
    version = "0.1.0",
    instructions = "Passive AirHound backend adapter. Only status, passive device inventory and session validation are supported. No capture control. Return backend evidence and uncertainty unchanged; do not infer radio readiness from validation or device presence."
)]
impl ServerHandler for AirHoundMcp {}

#[cfg(test)]
#[path = "../../../hostd/crates/airhound-client/src/test_clock.rs"]
mod test_clock;

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> Value {
        serde_json::to_value(airhound_api::Hello {
            version: airhound_api::VERSION,
            instance_id: "manual-clock".into(),
            build_version: "test".into(),
            max_frame_bytes: airhound_api::MAX_FRAME_BYTES,
            methods: vec![],
            mode: "passive_control".into(),
        })
        .unwrap()
    }

    async fn hello_and_status(stream: &mut tokio::net::UnixStream) {
        use airhound_client::{read_frame, write_frame};
        let request: airhound_api::Request =
            serde_json::from_slice(&read_frame(stream).await.unwrap()).unwrap();
        assert!(matches!(request.command, airhound_api::Command::Hello {}));
        write_frame(
            stream,
            &airhound_api::Response::success(request.request_id, hello()),
        )
        .await
        .unwrap();
        let request: airhound_api::Request =
            serde_json::from_slice(&read_frame(stream).await.unwrap()).unwrap();
        assert!(matches!(request.command, airhound_api::Command::Status {}));
        assert_eq!(request.request_id, 2);
    }

    #[tokio::test(start_paused = true)]
    async fn backend_timeout_preserves_error_identity_and_releases_connection() {
        use tokio::io::AsyncReadExt;
        let time = super::test_clock::ManualTime::hold().await;
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("control.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (seen, received) = tokio::sync::oneshot::channel();
        let peer = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            hello_and_status(&mut stream).await;
            seen.send(()).unwrap();
            assert_eq!(stream.read(&mut [0]).await.unwrap(), 0);
        });
        let server = AirHoundMcp::new(socket);
        let call = server.query(Query::Status);
        tokio::pin!(call);
        tokio::select! { _ = &mut call => panic!("early response"), _ = received => {} }
        tokio::time::advance(Duration::from_millis(19_999)).await;
        assert!(std::future::Future::poll(
            call.as_mut(),
            &mut std::task::Context::from_waker(std::task::Waker::noop())
        )
        .is_pending());
        tokio::time::advance(Duration::from_millis(1)).await;
        let result = call.await;
        assert_eq!(result.is_error, Some(true));
        let error = result.structured_content.unwrap();
        assert_eq!(error["error"]["code"], "backend_unavailable");
        assert_eq!(error["error"]["message"], "control request timed out");
        assert!(server.active.try_acquire().is_ok());
        peer.await.unwrap();
        time.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn overall_deadline_includes_handshake_time() {
        use tokio::io::AsyncReadExt;
        let time = super::test_clock::ManualTime::hold().await;
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("control.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (seen_hello, hello_received) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let (seen_status, status_received) = tokio::sync::oneshot::channel();
        let peer = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request: airhound_api::Request =
                serde_json::from_slice(&airhound_client::read_frame(&mut stream).await.unwrap())
                    .unwrap();
            assert!(matches!(request.command, airhound_api::Command::Hello {}));
            seen_hello.send(()).unwrap();
            released.await.unwrap();
            airhound_client::write_frame(
                &mut stream,
                &airhound_api::Response::success(request.request_id, hello()),
            )
            .await
            .unwrap();
            let request: airhound_api::Request =
                serde_json::from_slice(&airhound_client::read_frame(&mut stream).await.unwrap())
                    .unwrap();
            assert!(matches!(request.command, airhound_api::Command::Status {}));
            seen_status.send(()).unwrap();
            assert_eq!(stream.read(&mut [0]).await.unwrap(), 0);
        });
        let server = AirHoundMcp::new(socket);
        let call = server.query(Query::Status);
        tokio::pin!(call);
        tokio::select! { _ = &mut call => panic!("early response"), _ = hello_received => {} }
        tokio::time::advance(Duration::from_secs(15)).await;
        release.send(()).unwrap();
        tokio::select! { _ = &mut call => panic!("early response"), _ = status_received => {} }
        // The status request's own 20s timer has not expired at the overall 25s bound.
        tokio::time::advance(Duration::from_millis(9_999)).await;
        assert!(std::future::Future::poll(
            call.as_mut(),
            &mut std::task::Context::from_waker(std::task::Waker::noop())
        )
        .is_pending());
        tokio::time::advance(Duration::from_millis(1)).await;
        let result = call.await;
        assert_eq!(result.is_error, Some(true));
        let error = result.structured_content.unwrap();
        assert_eq!(error["error"]["code"], "backend_unavailable");
        assert_eq!(error["error"]["message"], "control request timed out");
        assert!(server.active.try_acquire().is_ok());
        peer.await.unwrap();
        time.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_backend_query_closes_peer_and_allows_fresh_query() {
        use tokio::io::AsyncReadExt;
        let time = super::test_clock::ManualTime::hold().await;
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("control.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (seen, received) = tokio::sync::oneshot::channel();
        let peer = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            hello_and_status(&mut first).await;
            seen.send(()).unwrap();
            assert_eq!(first.read(&mut [0]).await.unwrap(), 0);
            let (mut next, _) = listener.accept().await.unwrap();
            hello_and_status(&mut next).await;
            airhound_client::write_frame(
                &mut next,
                &airhound_api::Response::success(2, json!({"fresh":true})),
            )
            .await
            .unwrap();
        });
        let server = AirHoundMcp::new(socket);
        {
            let call = server.query(Query::Status);
            tokio::pin!(call);
            tokio::select! { _ = &mut call => panic!("early response"), _ = received => {} }
        }
        let result = server.query(Query::Status).await;
        assert_ne!(result.is_error, Some(true));
        assert_eq!(result.structured_content.unwrap()["fresh"], true);
        peer.await.unwrap();
        time.finish().await;
    }

    #[tokio::test]
    async fn overload_is_bounded_and_error_text_keeps_utf8_valid() {
        let server = AirHoundMcp::new(PathBuf::from("/unused/socket"));
        let permit = server.active.acquire().await.unwrap();
        let result = server.query(Query::Status).await;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.structured_content.unwrap()["error"]["code"], "busy");
        drop(permit);
        assert_eq!(bounded(&"🔎".repeat(4096)).chars().count(), MAX_ERROR_CHARS);
    }
}
