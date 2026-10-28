mod app;
mod config;
mod discovery;
mod library;
mod metadata;
mod player;
mod thumbnail;
mod ui;

use std::{io, panic, time::Duration};

use anyhow::Result;
use crossterm::{
    cursor::Show,
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use app::App;

fn main() -> Result<()> {
    let mut app = App::new()?;
    let hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        restore_terminal();
        hook(info);
    }));
    enable_raw_mode()?;
    if let Err(error) = execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste) {
        restore_terminal();
        return Err(error.into());
    }
    let result = run(&mut app);
    restore_terminal();
    result
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        DisableBracketedPaste,
        LeaveAlternateScreen,
        Show
    );
}

fn run(app: &mut App) -> Result<()> {
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut image_detection_done = false;
    while !app.quit {
        if app.settings.show_thumbnail && !image_detection_done {
            app.thumbnail.detect_terminal();
            image_detection_done = true;
        }
        app.poll_player();
        let previous_image_area = app.thumbnail.take_rendered_area();
        terminal.draw(|frame| ui::draw(frame, app))?;
        // Sixel pixels can survive buffer diffs when an image shrinks or a dialog hides it.
        if previous_image_area.is_some() && previous_image_area != app.thumbnail.rendered_area() {
            terminal.clear()?;
            terminal.draw(|frame| ui::draw(frame, app))?;
        }
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
                Event::Paste(text) => app.handle_paste(&text),
                Event::Resize(_, _) => terminal.clear()?,
                _ => {}
            }
        }
    }
    Ok(())
}
