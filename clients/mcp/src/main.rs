use airhound_mcp::{AirHoundMcp, MAX_MCP_LINE_BYTES};
use rmcp::{transport::async_rw::AsyncRwTransport, ServiceExt};
use std::path::PathBuf;

fn socket_argument() -> Result<PathBuf, &'static str> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--socket")) {
        return Err("usage: airhound-mcp --socket PATH");
    }
    let path = args
        .next()
        .filter(|value| !value.is_empty())
        .ok_or("--socket requires a path")?;
    if args.next().is_some() {
        return Err("usage: airhound-mcp --socket PATH");
    }
    Ok(path.into())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = AirHoundMcp::new(socket_argument()?);
    // The official SDK owns framing, initialization, tools and cancellation.
    // stdout is exclusively MCP; diagnostics from main's Result go to stderr.
    let transport = AsyncRwTransport::new(tokio::io::stdin(), tokio::io::stdout())
        .with_max_line_length(MAX_MCP_LINE_BYTES);
    server.serve(transport).await?.waiting().await?;
    Ok(())
}
