use airhound_mcp::{AirHoundMcp, MAX_MCP_LINE_BYTES};
use rmcp::{transport::async_rw::AsyncRwTransport, ServiceExt};
use std::path::PathBuf;

fn socket_argument(
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Result<PathBuf, &'static str> {
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
    let server = AirHoundMcp::new(socket_argument(std::env::args_os().skip(1))?);
    // The official SDK owns framing, initialization, tools and cancellation.
    // stdout is exclusively MCP; diagnostics from main's Result go to stderr.
    let transport = AsyncRwTransport::new(tokio::io::stdin(), tokio::io::stdout())
        .with_max_line_length(MAX_MCP_LINE_BYTES);
    server.serve(transport).await?.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_line_passes_only_the_explicit_socket() {
        let selected = PathBuf::from("/private/synthetic AirHound/control.sock");
        assert_eq!(
            socket_argument(["--socket".into(), selected.clone().into_os_string()].into_iter())
                .unwrap(),
            selected
        );
        for args in [
            vec![],
            vec!["--socket"],
            vec!["--socket", ""],
            vec!["--socket", "a", "b"],
            vec!["--other", "a"],
        ] {
            assert!(socket_argument(args.into_iter().map(Into::into)).is_err());
        }
    }
}
