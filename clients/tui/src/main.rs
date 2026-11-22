mod app;
mod backend;
use clap::Parser;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use std::{
    io::{self, IsTerminal},
    path::PathBuf,
};
use tokio::task::JoinSet;

#[derive(Parser)]
#[command(
    about = "Passive AirHound terminal client for an existing local service; never starts radios"
)]
struct Args {
    /// Explicit Unix socket owned by the running AirHound backend
    #[arg(long)]
    socket: PathBuf,
    /// Optional JSON session to validate through the backend (v); never starts it
    #[arg(long)]
    session: Option<PathBuf>,
}
enum Reply {
    Refresh(Result<backend::Snapshot, String>),
    Validate(Result<serde_json::Value, String>),
}
struct RestoreTerminal;
impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let args = Args::parse();
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "TUI requires interactive stdin and stdout; --help is available without a terminal",
        ));
    }
    // Ratatui installs its panic restoration hook before raw mode initialization.
    let mut terminal = ratatui::init();
    let _restore = RestoreTerminal;
    let mut app = app::App::default();
    let mut jobs = JoinSet::new();
    let socket = args.socket.clone();
    app.begin();
    jobs.spawn(async move { Reply::Refresh(backend::refresh(&socket).await) });
    let mut events = EventStream::new();
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    loop {
        let session = args.session.as_ref().map(|path| path.to_string_lossy());
        terminal.draw(|frame| app.draw(frame, session.as_deref()))?;
        tokio::select! {
            _=tokio::signal::ctrl_c()=>break,
            _=terminate.recv()=>break,
            _=hangup.recv()=>break,
            result=jobs.join_next(),if !jobs.is_empty()=>{
                match result {
                    Some(Ok(Reply::Refresh(result)))=>app.refreshed(result),
                    Some(Ok(Reply::Validate(result)))=>app.validated(result),
                    Some(Err(error))=>{app.busy=false;app.error=Some(format!("Request failed: {error}"));app.stale=true;},
                    None=>{}
                }
            },
            event=events.next()=>match event {
                Some(Ok(Event::Key(key))) if key.kind==KeyEventKind::Press=>{
                    match key.code {
                        KeyCode::Char('q')|KeyCode::Esc=>break,
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL)=>break,
                        KeyCode::Down|KeyCode::Char('j')=>app.navigate(true),
                        KeyCode::Up|KeyCode::Char('k')=>app.navigate(false),
                        KeyCode::PageDown=>app.scroll=app.scroll.saturating_add(10),
                        KeyCode::PageUp=>app.scroll=app.scroll.saturating_sub(10),
                        KeyCode::Tab=>{if app.show_status {app.show_status=false;app.show_validation=false;} else if app.show_validation {app.show_validation=false;app.show_status=true;} else {app.show_validation=true;} app.scroll=0;},
                        KeyCode::Char('r')|KeyCode::F(5) if !app.busy=>{let socket=args.socket.clone();app.begin();jobs.spawn(async move{Reply::Refresh(backend::refresh(&socket).await)});},
                        KeyCode::Char('v') if !app.busy=>{
                            if let Some(config)=args.session.clone(){let socket=args.socket.clone();app.begin();jobs.spawn(async move{Reply::Validate(backend::validate(&socket,&config).await)});}
                            else{app.error=Some("Launch with --session PATH to validate a session".into());}
                        },
                        _=>{}
                    }
                },
                Some(Ok(_))=>{},
                Some(Err(error))=>return Err(error),
                None=>break,
            }
        }
    }
    // Cancelling requests drops their sockets; the separately owned service stays up.
    jobs.abort_all();
    Ok(())
}
