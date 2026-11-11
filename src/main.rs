use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::Parser;
use crossterm::event::{self, Event, KeyEventKind};

mod app;
mod config;
mod git;
mod theme;
mod tree;
mod ui;

use app::{App, AppEvent};

/// Read-only terminal observer for agent-driven changes.
#[derive(Parser)]
#[command(name = "agtx", version, about)]
struct Args {
    /// Repository root to observe.
    #[arg(default_value = ".")]
    path: PathBuf,

    /// TOML config file to use instead of the default location.
    #[arg(long)]
    config: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config = config::load(args.config.as_deref())?;
    let root = git::repo_root(&args.path)?;
    let state = git::status::read(&root)?;
    let paths = git::files::visible(&root, &state)?;
    let mut app = App::new(root, config, state, paths);

    let mut terminal = ratatui::init();
    // Shared with the filesystem watcher in Task 5; kept alive here.
    let (tx, rx) = mpsc::channel::<AppEvent>();
    let result = run(&mut terminal, &mut app, &tx, &rx);
    ratatui::restore();
    result
}

/// Drawn only when `app.dirty`; keyboard events are routed through the
/// channel so the watcher can share it in Task 5.
fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    tx: &mpsc::Sender<AppEvent>,
    rx: &mpsc::Receiver<AppEvent>,
) -> Result<()> {
    let tick_rate = Duration::from_secs(1);
    let mut last_tick = Instant::now();
    loop {
        while let Ok(event) = rx.try_recv() {
            app.handle_event(event);
        }
        if app.should_quit() {
            return Ok(());
        }
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    if tx.send(AppEvent::Key(key)).is_err() {
                        break;
                    }
                }
                Event::Resize(_, _) => {
                    if tx.send(AppEvent::Resize).is_err() {
                        break;
                    }
                }
                _ => {}
            }
        }
        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
            if tx.send(AppEvent::Tick).is_err() {
                break;
            }
        }
        if app.dirty {
            terminal.draw(|f| ui::draw(f, app))?;
            app.dirty = false;
        }
    }
    Ok(())
}
