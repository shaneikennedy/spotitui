use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio;

mod app;
mod spotify;
mod ui;

use app::App;

static TERMINAL_INITIALIZED: AtomicBool = AtomicBool::new(false);

fn restore_terminal() {
    if TERMINAL_INITIALIZED.load(Ordering::SeqCst) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        TERMINAL_INITIALIZED.store(false, Ordering::SeqCst);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Set up signal handlers and panic hook
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        restore_terminal();
        r.store(false, Ordering::SeqCst);
        std::process::exit(0);
    })
    .expect("Error setting Ctrl-C handler");

    // Set up panic hook
    std::panic::set_hook(Box::new(|panic_info| {
        restore_terminal();
        eprintln!("Application panicked: {}", panic_info);
        std::process::exit(1);
    }));

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    TERMINAL_INITIALIZED.store(true, Ordering::SeqCst);

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run the application with proper error handling
    let app_result = run_app(&mut terminal).await;

    // Restore terminal
    restore_terminal();

    // Handle the result
    match app_result {
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("Application error: {}", e);
            std::process::exit(1);
        }
    }
}

async fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let mut app = match App::new().await {
        Ok(app) => app,
        Err(e) => {
            restore_terminal();
            eprintln!("Failed to initialize application: {}", e);
            eprintln!("Make sure you have set the SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET environment variables.");
            std::process::exit(1);
        }
    };

    app.run(terminal).await
}
