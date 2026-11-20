use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::Parser;
use crossterm::event::{self, Event, KeyEventKind};

mod app;
mod config;
mod editor;
mod git;
mod theme;
mod tree;
mod ui;
mod watch;

use app::{App, AppEvent};
use config::EditorProfile;

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
    let watcher_config = config.clone();
    let mut app = App::new(root, config, state, paths);

    let (tx, rx) = mpsc::channel::<AppEvent>();
    // Kept alive for the whole run: dropping it stops the watcher.
    let _watcher = match watch::spawn(app.root(), &watcher_config, tx.clone()) {
        Ok(watcher) => Some(watcher),
        Err(err) => {
            let _ = tx.send(AppEvent::Error(format!(
                "filesystem watcher disabled: {err:#}"
            )));
            None
        }
    };

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app, &tx, &rx);
    ratatui::restore();
    result
}

/// Leave the TUI, run the editor on the terminal, then always come back:
/// the terminal is re-initialized even when the launch failed.
fn launch_editor(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    profile: &EditorProfile,
    path: &Path,
) -> Result<()> {
    ratatui::restore();
    let launch_result = editor::launch(app.root(), profile, path, None);
    *terminal = ratatui::init();
    terminal.clear()?;
    match launch_result {
        Ok(()) => {
            if let Err(err) = app.refresh_git().and_then(|_| app.reload_diff()) {
                app.set_status(format!("error: {err:#}"));
            }
        }
        Err(err) => app.set_status(format!("error: {err:#}")),
    }
    app.dirty = true;
    terminal.draw(|frame| ui::draw(frame, app))?;
    app.dirty = false;
    Ok(())
}

/// Drawn only when `app.dirty`; keyboard events are routed through the
/// channel shared with the filesystem watcher.
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
        if let Some((profile, path)) = app.take_editor_request() {
            launch_editor(terminal, app, &profile, &path)?;
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
                Event::Resize(_, _) if tx.send(AppEvent::Resize).is_err() => break,
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
