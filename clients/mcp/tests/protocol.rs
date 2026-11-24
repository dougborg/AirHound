//! Actual stdio smoke plus controlled-time SDK forwarding integration.
//! The Unix backend is synthetic: no host scanners or receiver processes run.
use airhound_api::{
    ApiError, Command as BackendCommand, ErrorCode, Hello, Request, Response, MAX_FRAME_BYTES,
    METHODS, VERSION,
};
use airhound_client::{read_frame, write_frame};
use airhound_mcp::{AirHoundMcp, MAX_MCP_LINE_BYTES};

#[path = "../../../hostd/crates/airhound-client/src/test_clock.rs"]
mod test_clock;
use rmcp::{
    model::CallToolRequestParams,
    transport::{async_rw::AsyncRwTransport, TokioChildProcess},
    ServiceExt,
};
use serde_json::{json, Value};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{net::UnixListener, process::Command};

fn transport(socket: &Path) -> TokioChildProcess {
    let mut command = Command::new(env!("CARGO_BIN_EXE_airhound-mcp"));
    command.arg("--socket").arg(socket).stderr(Stdio::null());
    TokioChildProcess::new(command).unwrap()
}
fn hello() -> Value {
    serde_json::to_value(Hello {
        version: VERSION,
        instance_id: "fake-instance".into(),
        build_version: "test".into(),
        max_frame_bytes: MAX_FRAME_BYTES,
        methods: METHODS.iter().map(|m| (*m).into()).collect(),
        mode: "passive_control".into(),
    })
    .unwrap()
}
fn call(name: &'static str, args: Value) -> CallToolRequestParams {
    CallToolRequestParams::new(name).with_arguments(args.as_object().unwrap().clone())
}

#[tokio::test(start_paused = true)]
async fn sdk_forwarding_and_backend_errors_use_controlled_time() {
    let time = test_clock::ManualTime::hold().await;
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("backend.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let expected_config = json!({"version":1,"receivers":[{"id":"saved","source":{"kind":"wifi_pcap","path":"relative.pcap"}}]});
    let forwarded_config = expected_config.clone();
    let backend = tokio::spawn(async move {
        for expected in ["status", "devices", "validate", "invalid"] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request: Request =
                serde_json::from_slice(&read_frame(&mut stream).await.unwrap()).unwrap();
            assert!(matches!(request.command, BackendCommand::Hello {}));
            write_frame(&mut stream, &Response::success(request.request_id, hello()))
                .await
                .unwrap();
            let request: Request =
                serde_json::from_slice(&read_frame(&mut stream).await.unwrap()).unwrap();
            let response = match (expected, request.command) {
                ("status", BackendCommand::Status {}) => Response::success(
                    request.request_id,
                    json!({"mode":"passive_control","active_sessions":0}),
                ),
                ("devices", BackendCommand::DevicesList { all: false }) => Response::success(
                    request.request_id,
                    json!({"schema_version":1,"discovery_mode":"passive","devices":[{"id":"unknown","readiness":"unverified","future_field":42}]}),
                ),
                ("validate", BackendCommand::SessionValidate { config }) => {
                    assert_eq!(config, forwarded_config);
                    Response::success(
                        request.request_id,
                        json!({"valid":true,"readiness":"unverified","requested_assignment":{"mode":"replay"}}),
                    )
                }
                ("invalid", BackendCommand::SessionValidate { .. }) => Response::failure(
                    request.request_id,
                    ApiError::new(ErrorCode::InvalidSession, "bad config"),
                ),
                pair => panic!("unexpected backend request {pair:?}"),
            };
            write_frame(&mut stream, &response).await.unwrap();
        }
    });
    let (client_io, server_io) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        let (read, write) = tokio::io::split(server_io);
        AirHoundMcp::new(socket)
            .serve(AsyncRwTransport::new(read, write).with_max_line_length(MAX_MCP_LINE_BYTES))
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let client = ().serve(client_io).await.unwrap();
    let info = client.peer_info().unwrap();
    assert_eq!(info.server_info.as_ref().unwrap().name, "airhound");
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 3);
    for name in ["backend_status", "list_devices", "validate_session"] {
        let tool = tools.iter().find(|tool| tool.name == name).unwrap();
        assert_eq!(
            tool.annotations.as_ref().unwrap().read_only_hint,
            Some(true)
        );
        assert_eq!(
            tool.annotations.as_ref().unwrap().destructive_hint,
            Some(false)
        );
    }
    let status = client
        .call_tool(call("backend_status", json!({})))
        .await
        .unwrap();
    assert_eq!(status.structured_content.unwrap()["active_sessions"], 0);
    let devices = client
        .call_tool(call("list_devices", json!({})))
        .await
        .unwrap();
    assert_eq!(
        devices.structured_content.unwrap()["devices"][0],
        json!({"id":"unknown","readiness":"unverified","future_field":42})
    );
    let valid = client
        .call_tool(call("validate_session", json!({"config":expected_config})))
        .await
        .unwrap();
    assert_eq!(valid.structured_content.unwrap()["readiness"], "unverified");
    let invalid = client
        .call_tool(call("validate_session", json!({"config":{}})))
        .await
        .unwrap();
    assert_eq!(invalid.is_error, Some(true));
    assert_eq!(
        invalid.structured_content.unwrap()["error"]["code"],
        "invalid_session"
    );
    backend.await.unwrap();
    client.cancel().await.unwrap();
    server.await.unwrap();
    time.finish().await;
}

#[tokio::test(start_paused = true)]
async fn unavailable_backend_invalid_params_and_size_limits_are_errors() {
    let time = test_clock::ManualTime::hold().await;
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("absent.sock");
    let (client_io, server_io) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        let (read, write) = tokio::io::split(server_io);
        AirHoundMcp::new(socket)
            .serve(AsyncRwTransport::new(read, write).with_max_line_length(MAX_MCP_LINE_BYTES))
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let client = ().serve(client_io).await.unwrap();
    let result = client
        .call_tool(call("backend_status", json!({})))
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.structured_content.unwrap()["error"]["code"],
        "backend_unavailable"
    );
    let oversized = client
        .call_tool(call(
            "validate_session",
            json!({"config":{"large":"x".repeat(65536)}}),
        ))
        .await
        .unwrap();
    assert_eq!(oversized.is_error, Some(true));
    assert_eq!(
        oversized.structured_content.unwrap()["error"]["code"],
        "input_too_large"
    );
    let unknown = client
        .call_tool(call(
            "validate_session",
            json!({"config":{},"unknown":true}),
        ))
        .await;
    assert!(unknown.is_err() || unknown.unwrap().is_error == Some(true));
    let unsupported = client.call_tool(call("session_start", json!({}))).await;
    assert!(unsupported.is_err() || unsupported.unwrap().is_error == Some(true));
    client.cancel().await.unwrap();
    server.await.unwrap();
    time.finish().await;
}

/// Real executable/stdio smoke: only initialization and catalog operations,
/// which do not enter backend request deadlines. This is an outer hang guard,
/// not a deadline correctness assertion or backend timing test.
#[tokio::test]
async fn actual_stdio_executable_initializes_and_exposes_passive_tools() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let temp = tempfile::tempdir().unwrap();
        let client = ().serve(transport(&temp.path().join("explicit.sock"))).await.unwrap();
        assert_eq!(
            client
                .peer_info()
                .unwrap()
                .server_info
                .as_ref()
                .unwrap()
                .name,
            "airhound"
        );
        let tools = client.list_all_tools().await.unwrap();
        assert_eq!(tools.len(), 3);
        for name in ["backend_status", "list_devices", "validate_session"] {
            let tool = tools.iter().find(|tool| tool.name == name).unwrap();
            assert_eq!(
                tool.annotations.as_ref().unwrap().read_only_hint,
                Some(true)
            );
            assert_eq!(
                tool.annotations.as_ref().unwrap().destructive_hint,
                Some(false)
            );
        }
        client.cancel().await.unwrap();
    })
    .await
    .expect("stdio fixture outer hang guard");
}

#[test]
fn socket_is_explicit_and_stdout_is_protocol_only() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_airhound-mcp"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--socket PATH"));
}
