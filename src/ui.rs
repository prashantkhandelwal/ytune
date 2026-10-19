use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    symbols::border,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::app::{App, LibraryRow};

const ACCENT: Color = Color::Cyan;
const PLAYING: Color = Color::LightGreen;
const NOT_READY: Color = Color::Yellow;

#[derive(Clone, Copy)]
enum ShortcutMode {
    Playback,
    PlayerControls,
    Add,
    Resolving,
    Export,
    Import,
    Settings,
    MissingDependency,
    Delete,
}

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if area.width < 64 || area.height < 20 {
        frame.render_widget(
            Paragraph::new("ytune\nResize terminal to at least 64 x 20.\nq: quit")
                .block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    }
    let mode = if app.missing_ytdlp.is_some() {
        ShortcutMode::MissingDependency
    } else if app.delete_confirmation.is_some() {
        ShortcutMode::Delete
    } else if app.settings_form.is_some() {
        ShortcutMode::Settings
    } else if app.resolving() {
        ShortcutMode::Resolving
    } else if app.form.is_some() {
        ShortcutMode::Add
    } else if app.export_form.is_some() {
        ShortcutMode::Export
    } else if app.import_form.is_some() {
        ShortcutMode::Import
    } else {
        ShortcutMode::Playback
    };
    let shortcuts = shortcut_lines(mode, area.width.saturating_sub(2));
    let rows = Layout::vertical([
        Constraint::Length(
            if app.ready && app.error.is_some() && app.settings_form.is_none() {
                6
            } else {
                3
            },
        ),
        Constraint::Min(9),
        Constraint::Length(shortcuts.len() as u16 + 2),
    ])
    .split(area);
    let header = Block::default().borders(Borders::ALL);
    let header_inner = header.inner(rows[0]);
    frame.render_widget(header, rows[0]);
    let header_columns =
        Layout::horizontal([Constraint::Length(9), Constraint::Min(1)]).split(header_inner);
    frame.render_widget(
        Paragraph::new(" ytune ").style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        header_columns[0],
    );
    let (status, color) = if !app.ready {
        ("Not Ready", NOT_READY)
    } else if let Some(error) = &app.error {
        (error.as_str(), Color::LightRed)
    } else {
        (app.status.as_str(), Color::Gray)
    };
    frame.render_widget(
        Paragraph::new(status)
            .style(Style::default().fg(color))
            .alignment(Alignment::Right)
            .wrap(Wrap { trim: true }),
        header_columns[1],
    );

    let columns =
        Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)]).split(rows[1]);
    library(frame, app, columns[0]);
    details(frame, app, columns[1]);

    frame.render_widget(
        Paragraph::new(shortcuts).block(Block::default().borders(Borders::ALL).title(" Keyboard ")),
        rows[2],
    );
    if let Some(form) = &app.form {
        let width = area.width.saturating_sub(6).min(88);
        let height = 16.min(area.height);
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Add YouTube audio | Enter: save | Esc: cancel ")
            .border_style(Style::default().fg(ACCENT));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        let fields = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .split(inner);
        for (index, label) in [
            "Playlist / category",
            "Title override (optional)",
            "YouTube video URL",
        ]
        .iter()
        .enumerate()
        {
            let style = if index == form.focus {
                Style::default().fg(ACCENT)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            // Keep the end visible when a pasted URL exceeds the field width.
            let visible: String = form.fields[index]
                .chars()
                .rev()
                .take(usize::from(fields[index].width.saturating_sub(3)))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            frame.render_widget(
                Paragraph::new(visible).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(*label)
                        .border_style(style),
                ),
                fields[index],
            );
        }
        frame.render_widget(
            Paragraph::new(form.error.as_deref().unwrap_or(if app.resolving() {
                "Resolving title and metadata... Esc: cancel"
            } else {
                "Enter: resolve metadata and save | Tab: next field | Ctrl+U: clear"
            }))
            .style(Style::default().fg(if form.error.is_some() {
                Color::LightRed
            } else {
                Color::Gray
            }))
            .wrap(Wrap { trim: true }),
            fields[3],
        );
    }
    if let Some((form, importing)) = app
        .export_form
        .as_ref()
        .map(|form| (form, false))
        .or_else(|| app.import_form.as_ref().map(|form| (form, true)))
    {
        let width = area.width.saturating_sub(6).min(88);
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - 10) / 2,
            width,
            10,
        );
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(if importing {
                " Import playlists | Enter: import | Esc: cancel "
            } else {
                " Export entire library | Enter: export | Esc: cancel "
            })
            .border_style(Style::default().fg(ACCENT));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        let fields = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(inner);
        let visible: String = form
            .path
            .chars()
            .rev()
            .take(usize::from(fields[0].width.saturating_sub(3)))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        frame.render_widget(
            Paragraph::new(visible).block(Block::default().borders(Borders::ALL).title(
                if importing {
                    " Source JSON path "
                } else {
                    " Destination JSON path "
                },
            )),
            fields[0],
        );
        frame.render_widget(
            Paragraph::new(form.error.as_deref().unwrap_or(if importing {
                "Merge exported JSON into your library. Duplicate URLs in the same category are skipped. Ctrl+U: clear path"
            } else {
                "Includes every category and saved metadata. Existing files are not overwritten. Ctrl+U: clear path"
            })).wrap(Wrap { trim: true }).style(Style::default().fg(
                if form.error.is_some() { Color::LightRed } else { Color::Gray },
            )),
            fields[1],
        );
    }
    if app.settings_form.is_some() {
        settings_dialog(frame, app, rows[1]);
    }
    if app.player_shortcuts_open {
        let width = area.width.saturating_sub(6).min(70);
        let shortcuts = shortcut_lines(ShortcutMode::PlayerControls, width - 2);
        let height = shortcuts.len() as u16 + 2;
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ACCENT))
            .title(" Player shortcuts | k / Esc: close ");
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        frame.render_widget(Paragraph::new(shortcuts), inner);
    }
    if let Some(confirmation) = &app.delete_confirmation {
        let width = area.width.saturating_sub(6).min(70);
        let height = 14;
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(" Delete song? ");
        let inner = block.inner(popup).inner(Margin::new(2, 1));
        frame.render_widget(block, popup);
        let sections = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(2),
            Constraint::Length(1),
        ])
        .split(inner);
        frame.render_widget(
            Paragraph::new("Remove this song from your playlist?").alignment(Alignment::Center),
            sections[0],
        );
        frame.render_widget(
            Paragraph::new(confirmation.track.title.as_str())
                .style(
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            sections[2],
        );
        frame.render_widget(
            Paragraph::new(format!("Category: {}", confirmation.category))
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            sections[3],
        );
        frame.render_widget(
            Paragraph::new(
                confirmation
                    .error
                    .as_deref()
                    .unwrap_or("This removes the song from the saved playlist."),
            )
            .style(Style::default().fg(if confirmation.error.is_some() {
                Color::LightRed
            } else {
                Color::Gray
            }))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
            sections[4],
        );
        let buttons = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(sections[5]);
        for (index, (key, label)) in [("y", "Yes, delete"), ("n / Esc", "No, keep")]
            .into_iter()
            .enumerate()
        {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        format!(" {key} "),
                        Style::default()
                            .fg(Color::Black)
                            .bg(ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" {label} "),
                        Style::default().fg(Color::White).bg(Color::DarkGray),
                    ),
                ]))
                .alignment(Alignment::Center),
                buttons[index],
            );
        }
    }
    if let Some(message) = &app.missing_ytdlp {
        let width = area.width.saturating_sub(6).min(80);
        let height = 12;
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::LightRed))
            .title(" Missing yt-dlp ");
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        let sections = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
        frame.render_widget(
            Paragraph::new(message.as_str())
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true })
                .style(Style::default().fg(Color::LightRed)),
            sections[0],
        );
        frame.render_widget(
            Paragraph::new("t: open settings | Esc: manage library | q: quit")
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            sections[1],
        );
    }
}

fn settings_dialog(frame: &mut Frame, app: &App, area: Rect) {
    let Some(form) = &app.settings_form else {
        return;
    };
    let width = area.width.saturating_sub(6).min(88);
    let height = area.height.min(18);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(" Settings | Enter: save | Esc: cancel ");
    if height < 14 {
        block = block.title_bottom(Line::styled(
            form.error
                .as_deref()
                .unwrap_or("Blank executable: automatic discovery"),
            Style::default().fg(if form.error.is_some() {
                Color::LightRed
            } else {
                Color::Gray
            }),
        ));
    }
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let sections = Layout::vertical([Constraint::Length(11), Constraint::Min(0)]).split(inner);
    let columns =
        Layout::horizontal([Constraint::Length(20), Constraint::Min(1)]).split(sections[0]);
    let labels = [
        "yt-dlp executable",
        "mpv executable",
        "Seek interval (s)",
        "Volume step (%)",
        "Data folder",
    ];
    // Shared edges keep five bordered inputs visible in a 64 x 20 terminal.
    for index in (0..labels.len())
        .filter(|&index| index != form.focus)
        .chain(std::iter::once(form.focus))
    {
        let focused = index == form.focus;
        let input = Rect::new(
            columns[1].x,
            columns[1].y + index as u16 * 2,
            columns[1].width,
            3,
        );
        let input_border = border::Set {
            top_left: if index == 0 {
                border::PLAIN.top_left
            } else {
                "\u{251c}"
            },
            top_right: if index == 0 {
                border::PLAIN.top_right
            } else {
                "\u{2524}"
            },
            bottom_left: if index == labels.len() - 1 {
                border::PLAIN.bottom_left
            } else {
                "\u{251c}"
            },
            bottom_right: if index == labels.len() - 1 {
                border::PLAIN.bottom_right
            } else {
                "\u{2524}"
            },
            ..border::PLAIN
        };
        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_set(input_border)
            .border_style(Style::default().fg(if focused { ACCENT } else { Color::DarkGray }));
        let input_inner = input_block.inner(input);
        let value: String = form.fields[index]
            .chars()
            .rev()
            .take(usize::from(input_inner.width.saturating_sub(1)))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        frame.render_widget(
            Paragraph::new(format!("{}:", labels[index])).style(
                Style::default()
                    .fg(if focused { ACCENT } else { Color::Gray })
                    .add_modifier(if focused {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Rect::new(columns[0].x, input.y + 1, columns[0].width, 1),
        );
        frame.render_widget(
            Paragraph::new(value)
                .block(input_block)
                .style(Style::default().fg(Color::White).bg(if focused {
                    Color::DarkGray
                } else {
                    Color::Black
                })),
            input,
        );
    }
    frame.render_widget(
        Paragraph::new(form.error.as_deref().unwrap_or(
            "Blank executable: auto. Seek > 0; volume 1-100. Folder changes copy data; executable changes stop playback.",
        )).wrap(Wrap { trim: true }).style(Style::default().fg(
            if form.error.is_some() { Color::LightRed } else { Color::Gray },
        )),
        sections[1],
    );
}

fn shortcut_lines(mode: ShortcutMode, width: u16) -> Vec<Line<'static>> {
    let shortcuts: &[(&str, &str)] = match mode {
        ShortcutMode::Add => &[
            ("Tab/Shift+Tab", "Field"),
            ("Enter", "Save"),
            ("Esc", "Cancel"),
            ("Ctrl+U", "Clear"),
        ],
        ShortcutMode::Resolving => &[("Esc", "Cancel lookup")],
        ShortcutMode::Export => &[("Enter", "Export"), ("Esc", "Cancel"), ("Ctrl+U", "Clear")],
        ShortcutMode::Import => &[("Enter", "Import"), ("Esc", "Cancel"), ("Ctrl+U", "Clear")],
        ShortcutMode::Settings => &[
            ("Tab/Shift+Tab", "Field"),
            ("Enter", "Save"),
            ("Esc", "Cancel"),
            ("Ctrl+U", "Clear"),
        ],
        ShortcutMode::MissingDependency => &[("t", "Settings"), ("Esc", "Dismiss"), ("q", "Quit")],
        ShortcutMode::Delete => &[("y", "Delete"), ("n/Esc", "Cancel"), ("q", "Quit")],
        ShortcutMode::PlayerControls => &[
            ("Up/Down", "Select"),
            ("Space", "Play/Pause"),
            ("n/b", "Next/Prev"),
            ("Left/Right", "Seek"),
            ("+/-", "Volume"),
            ("r", "Repeat"),
            ("m", "Mute"),
            ("s", "Stop"),
        ],
        ShortcutMode::Playback => &[
            ("k", "Player controls"),
            ("a", "Add"),
            ("e", "Export"),
            ("i", "Import"),
            ("t", "Settings"),
            ("Tab", "Expand/Collapse"),
            ("Del", "Remove"),
            ("q", "Quit"),
        ],
    };
    let key_style = Style::default()
        .fg(Color::Black)
        .bg(ACCENT)
        .add_modifier(Modifier::BOLD);
    let action_style = Style::default().fg(Color::White).bg(Color::DarkGray);
    let mut lines = Vec::new();
    let mut line = Line::default();
    for (index, (key, action)) in shortcuts.iter().enumerate() {
        let key = Span::styled(format!(" {key} "), key_style);
        let action = Span::styled(format!(" {action} "), action_style);
        let badge_width = key.width() + action.width();
        if !line.spans.is_empty() {
            if matches!(mode, ShortcutMode::PlayerControls) {
                if index % 2 == 0 {
                    lines.push(line);
                    lines.push(Line::default());
                    line = Line::default();
                } else {
                    let padding = usize::from(width / 2).saturating_sub(line.width());
                    line.spans.push(Span::raw(" ".repeat(padding)));
                }
            } else if line.width() + 1 + badge_width > usize::from(width) {
                lines.push(line);
                line = Line::default();
            } else {
                line.spans.push(Span::raw(" "));
            }
        }
        line.spans.extend([key, action]);
    }
    if !line.spans.is_empty() {
        lines.push(line);
    }
    lines
}

fn library(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Library | {} tracks ", app.library.len()));
    if app.library.len() == 0 {
        frame.render_widget(
            Paragraph::new(
                "Your library is empty.\n\nPress a to add a YouTube URL.\nGive it a playlist or category name to group your audio.",
            )
            .block(block)
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let mut items = Vec::new();
    let mut selected_row = None;
    for row in app.visible_rows() {
        if row == app.selected_row() {
            selected_row = Some(items.len());
        }
        match row {
            LibraryRow::Category(index) => {
                let playlist = &app.library.playlists[index];
                let collapsed = app.collapsed.contains(&playlist.name);
                let current = app.playback.active && app.playback.category == playlist.name;
                items.push(ListItem::new(Line::styled(
                    format!(
                        " [{}] {} ({})",
                        if collapsed { "+" } else { "-" },
                        playlist.name,
                        playlist.tracks.len()
                    ),
                    Style::default()
                        .fg(if current && collapsed {
                            PLAYING
                        } else {
                            ACCENT
                        })
                        .add_modifier(Modifier::BOLD),
                )));
            }
            LibraryRow::Track(index) => {
                let Some((category, track)) = app.library.get(index) else {
                    continue;
                };
                let current = app.playback.category == category
                    && app
                        .playback
                        .track
                        .as_ref()
                        .is_some_and(|t| t.url == track.url);
                let active = current && app.playback.active;
                let marker = if active { ">" } else { " " };
                let style = if active {
                    Style::default()
                        .fg(if app.playback.paused {
                            Color::Yellow
                        } else {
                            PLAYING
                        })
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                items.push(ListItem::new(format!("{marker} {}", track.title)).style(style));
            }
        }
    }
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol(">> ");
    frame.render_stateful_widget(
        list,
        area,
        &mut ListState::default().with_selected(selected_row),
    );
}

fn details(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Audio details ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let sections = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).split(inner);
    let playing = &app.playback;
    let selected = app.library.get(app.selected);
    let track = playing.track.as_ref().or_else(|| selected.map(|(_, t)| t));
    let stored = track.and_then(|track| track.metadata.as_ref());
    let artist = if playing.artist.is_empty() {
        stored.map(|m| m.artist_label()).unwrap_or("")
    } else {
        &playing.artist
    };
    let album = if playing.album.is_empty() {
        stored.and_then(|m| m.album.as_deref()).unwrap_or("")
    } else {
        &playing.album
    };
    let duration = if playing.track.is_some() {
        playing.duration
    } else {
        stored.and_then(|m| m.duration).unwrap_or_default()
    };
    let category = if playing.track.is_some() {
        playing.category.as_str()
    } else {
        selected.map(|(category, _)| category).unwrap_or("-")
    };
    let title = if !playing.title.is_empty() {
        playing.title.as_str()
    } else {
        track.map(|t| t.title.as_str()).unwrap_or("Nothing playing")
    };
    let state = if playing.loading {
        "Loading..."
    } else if !playing.active {
        "Stopped"
    } else if playing.paused {
        "Paused"
    } else {
        "Playing"
    };
    let lines = vec![
        Line::styled(
            title,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::raw(format!("State:    {state}")),
        Line::raw(format!("Category: {category}")),
        Line::raw(format!("Artist:   {}", nonempty(artist))),
        Line::raw(format!("Album:    {}", nonempty(album))),
        Line::raw(format!(
            "Volume:   {:.0}%{}",
            app.volume,
            if app.muted { " (muted)" } else { "" }
        )),
        Line::raw(format!("Repeat:   {}", app.repeat.label())),
        Line::raw(""),
        Line::styled("Source", Style::default().fg(Color::Gray)),
        Line::raw(
            track
                .map(|t| t.url.as_str())
                .unwrap_or("Add a video with a"),
        ),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), sections[0]);
    seek_bar(frame, sections[1], playing.position, duration);
}

fn seek_bar(frame: &mut Frame, area: Rect, position: f64, duration: f64) {
    let block = Block::default().borders(Borders::TOP);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(inner);
    let ratio = if duration > 0.0 {
        (position / duration).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let last = usize::from(inner.width.saturating_sub(1));
    let playhead = (ratio * last as f64).round() as usize;
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("\u{2500}".repeat(playhead), Style::default().fg(ACCENT)),
            Span::styled(
                "\u{2588}",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "\u{2500}".repeat(last - playhead),
                Style::default().fg(Color::DarkGray),
            ),
        ])),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(format!(
            "{} / {}",
            time(position),
            if duration > 0.0 {
                time(duration)
            } else {
                "LIVE / unknown".into()
            },
        ))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::Gray).bg(Color::Reset)),
        rows[1],
    );
}

fn nonempty(value: &str) -> &str {
    if value.is_empty() { "-" } else { value }
}

fn time(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_label_is_gray_without_changing_url_color() -> anyhow::Result<()> {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut app, _directory, _commands, _events) = crate::app::tests::fixture();
        let url = "https://youtu.be/one";
        app.library.add("Focus", "Test track", url)?;
        let mut terminal = Terminal::new(TestBackend::new(80, 20))?;
        terminal.draw(|frame| details(frame, &app, frame.area()))?;
        let buffer = terminal.backend().buffer();
        for (index, character) in "Source".chars().enumerate() {
            let cell = &buffer[(1 + index as u16, 10)];
            assert_eq!(cell.symbol(), character.to_string());
            assert_eq!(cell.fg, Color::Gray);
        }
        for (index, character) in url.chars().enumerate() {
            let cell = &buffer[(1 + index as u16, 11)];
            assert_eq!(cell.symbol(), character.to_string());
            assert_eq!(cell.fg, Color::Reset);
        }
        Ok(())
    }

    #[test]
    fn renders_library_details_shortcuts_and_add_dialog() -> anyhow::Result<()> {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut app, _directory, _commands, _events) = crate::app::tests::fixture();
        app.library
            .add("Focus", "Test track", "https://youtu.be/one")?;
        let mut terminal = Terminal::new(TestBackend::new(110, 30))?;
        terminal.draw(|frame| draw(frame, &app))?;
        let content = format!("{}", terminal.backend());
        for expected in [
            "Focus (1)",
            "Test track",
            "Audio details",
            "k  Player controls",
            "Repeat:",
        ] {
            assert!(content.contains(expected), "Missing UI text: {expected}");
        }
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('a'),
            crossterm::event::KeyModifiers::NONE,
        ));
        terminal.draw(|frame| draw(frame, &app))?;
        assert!(format!("{}", terminal.backend()).contains("YouTube video URL"));
        for (width, height) in [(64, 20), (20, 5), (1, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height))?;
            terminal.draw(|frame| draw(frame, &app))?;
        }
        app.form = None;
        let mut terminal = Terminal::new(TestBackend::new(64, 20))?;
        terminal.draw(|frame| draw(frame, &app))?;
        let content = format!("{}", terminal.backend());
        for expected in [
            "k  Player controls",
            "Del  Remove",
            "q  Quit",
            "e  Export",
            "i  Import",
            "t  Settings",
            "Tab  Expand/Collapse",
        ] {
            assert!(
                content.contains(expected),
                "Shortcut hidden at minimum size: {expected}"
            );
        }
        Ok(())
    }

    #[test]
    fn player_shortcuts_move_into_centered_dialog_without_hiding_other_shortcuts()
    -> anyhow::Result<()> {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut app, _directory, _commands, _events) = crate::app::tests::fixture();
        for (width, height) in [(64, 20), (110, 30)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height))?;
            app.player_shortcuts_open = false;
            terminal.draw(|frame| draw(frame, &app))?;
            let content = format!("{}", terminal.backend());
            let controls = [
                "Up/Down  Select",
                "+/-  Volume",
                "Space  Play/Pause",
                "n/b  Next/Prev",
                "Left/Right  Seek",
                "r  Repeat",
                "m  Mute",
                "s  Stop",
            ];
            for control in controls {
                assert!(!content.contains(control), "Still in footer: {control}");
            }
            for expected in [
                "k  Player controls",
                "e  Export",
                "i  Import",
                "t  Settings",
                "q  Quit",
            ] {
                assert!(
                    content.contains(expected),
                    "Missing footer shortcut: {expected}"
                );
            }
            app.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('k'),
                crossterm::event::KeyModifiers::NONE,
            ));
            terminal.draw(|frame| draw(frame, &app))?;
            let content = format!("{}", terminal.backend());
            for expected in controls.into_iter().chain(["Player shortcuts"]) {
                assert!(content.contains(expected), "Missing UI: {expected}");
            }
            let popup_width = (width - 6).min(70);
            let popup_height =
                shortcut_lines(ShortcutMode::PlayerControls, popup_width - 2).len() as u16 + 2;
            let buffer = terminal.backend().buffer();
            assert_eq!(
                buffer[((width - popup_width) / 2, (height - popup_height) / 2)].symbol(),
                "\u{250c}"
            );
            let left = (width - popup_width) / 2 + 2;
            let right = left + (popup_width - 2) / 2;
            let top = (height - popup_height) / 2 + 1;
            for (row, (left_key, right_key)) in [("U", "S"), ("n", "L"), ("+", "r"), ("m", "s")]
                .into_iter()
                .enumerate()
            {
                let y = top + row as u16 * 2;
                assert_eq!(buffer[(left, y)].symbol(), left_key);
                assert_eq!(buffer[(right, y)].symbol(), right_key);
                if row < 3 {
                    assert_eq!(buffer[(left, y + 1)].symbol(), " ");
                    assert_eq!(buffer[(right, y + 1)].symbol(), " ");
                }
            }
        }
        Ok(())
    }

    #[test]
    fn delete_confirmation_shows_song_category_and_save_errors() -> anyhow::Result<()> {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use ratatui::{Terminal, backend::TestBackend};

        let (mut app, _directory, _commands, _events) = crate::app::tests::fixture();
        app.library
            .add("Focus", "Test song", "https://youtu.be/one")?;
        app.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        for (width, height) in [(64, 20), (110, 30)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height))?;
            terminal.draw(|frame| draw(frame, &app))?;
            let content = format!("{}", terminal.backend());
            for expected in [
                "Delete song?",
                "Test song",
                "Category: Focus",
                "y  Yes, delete",
                "n / Esc  No, keep",
            ] {
                assert!(
                    content.contains(expected),
                    "Missing confirmation UI: {expected}"
                );
            }
            let x = (width - (width - 6).min(70)) / 2;
            let y = (height - 14) / 2;
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer[(x, y)].symbol(), "\u{250c}");
            let popup_width = (width - 6).min(70);
            let button_row: String = (x + 1..x + popup_width - 1)
                .map(|column| buffer[(column, y + 11)].symbol())
                .collect();
            assert!(button_row.contains("y  Yes, delete"));
            assert!(button_row.contains("n / Esc  No, keep"));
            assert!(
                (x + 1..x + popup_width - 1).all(|column| buffer[(column, y + 3)].symbol() == " ")
            );
        }
        app.delete_confirmation.as_mut().unwrap().error =
            Some("Cannot remove track: Access denied".into());
        let mut terminal = Terminal::new(TestBackend::new(64, 20))?;
        terminal.draw(|frame| draw(frame, &app))?;
        assert!(format!("{}", terminal.backend()).contains("Cannot remove track: Access denied"));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        terminal.draw(|frame| draw(frame, &app))?;
        assert!(!format!("{}", terminal.backend()).contains("Delete song?"));
        Ok(())
    }

    #[test]
    fn status_is_top_right_and_library_uses_thirty_percent() -> anyhow::Result<()> {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut app, _directory, _commands, _events) = crate::app::tests::fixture();
        let mut terminal = Terminal::new(TestBackend::new(110, 30))?;
        terminal.draw(|frame| draw(frame, &app))?;
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(104, 1)].symbol(), "R");
        assert_eq!(buffer[(108, 1)].symbol(), "y");
        assert_eq!(buffer[(33, 3)].symbol(), "┌");
        assert!(!format!("{}", terminal.backend()).contains(" Status "));

        app.error = Some("Playback failed".into());
        terminal.draw(|frame| draw(frame, &app))?;
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(94, 1)].symbol(), "P");
        assert_eq!(buffer[(94, 1)].fg, Color::LightRed);
        assert_eq!(buffer[(33, 6)].symbol(), "┌");
        Ok(())
    }

    #[test]
    fn shortcut_badges_have_distinct_backgrounds_and_wrap_without_splitting() {
        for mode in [
            ShortcutMode::Playback,
            ShortcutMode::PlayerControls,
            ShortcutMode::Add,
            ShortcutMode::Resolving,
            ShortcutMode::Export,
            ShortcutMode::Import,
            ShortcutMode::Settings,
            ShortcutMode::MissingDependency,
            ShortcutMode::Delete,
        ] {
            for width in [56, 62, 78, 108, 158] {
                let lines = shortcut_lines(mode, width);
                if matches!(mode, ShortcutMode::PlayerControls) {
                    assert_eq!(lines.len(), 7);
                    for (index, line) in lines.iter().enumerate() {
                        if index % 2 == 0 {
                            assert_eq!(line.spans.len(), 5);
                            assert_eq!(
                                line.spans[..3].iter().map(Span::width).sum::<usize>(),
                                usize::from(width / 2)
                            );
                        } else {
                            assert!(line.spans.is_empty());
                        }
                    }
                }
                assert!(
                    !lines
                        .iter()
                        .flat_map(|line| &line.spans)
                        .any(|span| span.content.contains("Ctrl+C"))
                );
                assert_eq!(
                    lines
                        .iter()
                        .flat_map(|line| &line.spans)
                        .any(|span| span.content == " Enter "),
                    matches!(
                        mode,
                        ShortcutMode::Add
                            | ShortcutMode::Export
                            | ShortcutMode::Import
                            | ShortcutMode::Settings
                    ),
                    "Enter should only be shown for saving, exporting, importing, or settings"
                );
                assert!(lines.iter().all(|line| line.width() <= usize::from(width)));
                for line in lines {
                    let badges: Vec<_> = line
                        .spans
                        .iter()
                        .filter(|span| span.style.bg.is_some())
                        .collect();
                    assert_eq!(badges.len() % 2, 0);
                    for badge in badges.chunks_exact(2) {
                        assert_eq!(badge[0].style.bg, Some(ACCENT));
                        assert_eq!(badge[1].style.bg, Some(Color::DarkGray));
                        assert_eq!(badge[0].style.fg, Some(Color::Black));
                        assert_eq!(badge[1].style.fg, Some(Color::White));
                    }
                }
            }
        }
    }

    #[test]
    fn playing_color_survives_selection_and_collapsed_categories_hide_tracks() -> anyhow::Result<()>
    {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut app, _directory, _commands, _events) = crate::app::tests::fixture();
        app.library.add("Focus", "One", "https://youtu.be/one")?;
        app.library.add("Focus", "Two", "https://youtu.be/two")?;
        app.playback.category = "Focus".into();
        app.playback.track = Some(app.library.get(0).unwrap().1.clone());
        app.playback.active = true;
        let mut terminal = Terminal::new(TestBackend::new(110, 30))?;
        for selected in [0, 1] {
            app.selected = selected;
            terminal.draw(|frame| draw(frame, &app))?;
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer[(6, 5)].symbol(), "O");
            assert_eq!(buffer[(6, 5)].fg, PLAYING);
            if selected == 0 {
                assert_eq!(buffer[(6, 5)].bg, Color::DarkGray);
            }
        }
        app.playback.paused = true;
        terminal.draw(|frame| draw(frame, &app))?;
        assert_eq!(terminal.backend().buffer()[(6, 5)].fg, Color::Yellow);
        app.playback.paused = false;
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));
        terminal.draw(|frame| draw(frame, &app))?;
        let buffer = terminal.backend().buffer();
        let left: String = (3..25)
            .flat_map(|y| (0..33).map(move |x| buffer[(x, y)].symbol().to_string()))
            .collect();
        assert!(left.contains("[+] Focus (2)"));
        assert!(!left.contains("One"));
        assert!(!left.contains("Two"));
        assert_eq!(buffer[(5, 4)].fg, PLAYING);
        Ok(())
    }

    #[test]
    fn renders_export_dialog_and_saved_metadata_preview() -> anyhow::Result<()> {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut app, _directory, _commands, _events) = crate::app::tests::fixture();
        app.library.add_resolved(
            "Focus",
            "",
            "https://youtu.be/one",
            crate::metadata::tests::sample(),
        )?;
        let mut terminal = Terminal::new(TestBackend::new(110, 30))?;
        terminal.draw(|frame| draw(frame, &app))?;
        let content = format!("{}", terminal.backend());
        for expected in [
            "Resolved title",
            "Artist:   Artist",
            "Album:    Album",
            "0:00 / 2:00",
        ] {
            assert!(
                content.contains(expected),
                "Missing saved metadata: {expected}"
            );
        }
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('e'),
            crossterm::event::KeyModifiers::NONE,
        ));
        terminal.draw(|frame| draw(frame, &app))?;
        let content = format!("{}", terminal.backend());
        assert!(content.contains("Export entire library"));
        assert!(content.contains("Destination JSON path"));
        assert!(content.contains("Enter  Export"));
        let mut small = Terminal::new(TestBackend::new(64, 20))?;
        small.draw(|frame| draw(frame, &app))?;
        Ok(())
    }

    #[test]
    fn renders_import_dialog_and_its_shortcuts_at_supported_sizes() -> anyhow::Result<()> {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut app, _directory, _commands, _events) = crate::app::tests::fixture();
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('i'),
            crossterm::event::KeyModifiers::NONE,
        ));
        app.handle_paste("C:\\playlists\\saved.json");
        for (width, height) in [(110, 30), (64, 20)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height))?;
            terminal.draw(|frame| draw(frame, &app))?;
            let content = format!("{}", terminal.backend());
            for expected in [
                "Import playlists",
                "Source JSON path",
                "C:\\playlists\\saved.json",
                "Enter  Import",
            ] {
                assert!(content.contains(expected), "Missing import UI: {expected}");
            }
        }
        app.import_form.as_mut().unwrap().error = Some("Invalid playlist JSON".into());
        let mut terminal = Terminal::new(TestBackend::new(110, 30))?;
        terminal.draw(|frame| draw(frame, &app))?;
        assert!(format!("{}", terminal.backend()).contains("Invalid playlist JSON"));
        Ok(())
    }

    #[test]
    fn missing_ytdlp_error_is_centered_with_yellow_not_ready_status() -> anyhow::Result<()> {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut app, _directory, _commands, events) = crate::app::tests::fixture();
        events.send(crate::player::PlayerEvent::YtdlpMissing(
            "yt-dlp was not found on PATH.\n\nInstall yt-dlp or set YTUNE_YTDLP, then restart ytune.".into(),
        ))?;
        app.poll_player();
        for (width, height) in [(110, 30), (64, 20)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height))?;
            terminal.draw(|frame| draw(frame, &app))?;
            let buffer = terminal.backend().buffer();
            let status_x = width - 10;
            assert_eq!(buffer[(status_x, 1)].symbol(), "N");
            assert_eq!(buffer[(status_x, 1)].fg, NOT_READY);
            let popup_width = width.saturating_sub(6).min(80);
            let popup_x = (width - popup_width) / 2;
            let popup_y = (height - 12) / 2;
            assert_eq!(buffer[(popup_x, popup_y)].symbol(), "\u{250c}");
            assert_eq!(buffer[(popup_x, popup_y)].fg, Color::LightRed);
            assert_eq!(
                buffer[(popup_x + popup_width - 1, popup_y + 11)].symbol(),
                "\u{2518}"
            );
            let content = format!("{}", terminal.backend());
            for expected in [
                "Missing yt-dlp",
                "not found on PATH",
                "YTUNE_YTDLP",
                "Esc  Dismiss",
            ] {
                assert!(content.contains(expected), "Missing error UI: {expected}");
            }
        }
        app.missing_ytdlp = None;
        app.status = "Library exported".into();
        let mut terminal = Terminal::new(TestBackend::new(110, 30))?;
        terminal.draw(|frame| draw(frame, &app))?;
        let content = format!("{}", terminal.backend());
        assert!(content.contains("Not Ready"));
        assert!(!content.contains("Missing yt-dlp"));
        assert!(!content.contains("Library exported"));
        Ok(())
    }

    #[test]
    fn settings_is_a_centered_overlay_with_visible_fields_shortcuts_and_errors()
    -> anyhow::Result<()> {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut app, _directory, _commands, _events) = crate::app::tests::fixture();
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('t'),
            crossterm::event::KeyModifiers::NONE,
        ));
        for (width, height) in [(110, 30), (64, 20)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height))?;
            terminal.draw(|frame| draw(frame, &app))?;
            let content = format!("{}", terminal.backend());
            for expected in [
                " Settings ",
                "yt-dlp executable",
                "mpv executable",
                "Seek interval",
                "Volume step",
                "Data folder",
                "Enter  Save",
                "Esc  Cancel",
            ] {
                assert!(
                    content.contains(expected),
                    "Missing settings UI: {expected}"
                );
            }
            if width == 110 {
                assert!(content.contains("Audio details"));
                assert!(content.contains(" Library "));
            }
            let footer_height = shortcut_lines(ShortcutMode::Settings, width - 2).len() as u16 + 2;
            let body_height = height - 3 - footer_height;
            let popup_x = (width - (width - 6).min(88)) / 2;
            let popup_y = 3 + (body_height - body_height.min(18)) / 2;
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer[(popup_x, popup_y)].symbol(), "\u{250c}");
            assert_eq!(buffer[(popup_x, popup_y)].fg, ACCENT);
            assert_eq!(buffer[(0, 3)].symbol(), "\u{250c}");
            assert_eq!(buffer[(width - 1, 3)].symbol(), "\u{2510}");
        }
        app.settings_form.as_mut().unwrap().error = Some("Seek interval must be positive".into());
        let mut terminal = Terminal::new(TestBackend::new(64, 20))?;
        terminal.draw(|frame| draw(frame, &app))?;
        assert!(format!("{}", terminal.backend()).contains("Seek interval must be positive"));
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        terminal.draw(|frame| draw(frame, &app))?;
        let content = format!("{}", terminal.backend());
        assert!(content.contains("Audio details"));
        assert!(content.contains(" Library "));
        assert!(!content.contains("yt-dlp executable"));
        Ok(())
    }

    #[test]
    fn settings_labels_and_bordered_values_share_rows_and_focus_is_visible() -> anyhow::Result<()> {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut app, _directory, _commands, _events) = crate::app::tests::fixture();
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('t'),
            crossterm::event::KeyModifiers::NONE,
        ));
        app.settings_form.as_mut().unwrap().fields = [
            "C:\\yt-dlp.exe".into(),
            "C:\\mpv.exe".into(),
            "25".into(),
            "7".into(),
            "C:\\Music".into(),
        ];
        for (width, height) in [(110, 30), (64, 20)] {
            app.settings_form.as_mut().unwrap().focus = 0;
            let mut terminal = Terminal::new(TestBackend::new(width, height))?;
            terminal.draw(|frame| draw(frame, &app))?;
            let footer = shortcut_lines(ShortcutMode::Settings, width - 2).len() as u16 + 2;
            let body_height = height - 3 - footer;
            let popup_x = (width - (width - 6).min(88)) / 2;
            let popup_y = 3 + (body_height - body_height.min(18)) / 2;
            let label_x = popup_x + 1;
            let border_x = popup_x + 21;
            let value_x = border_x + 1;
            let first_row = popup_y + 2;
            let buffer = terminal.backend().buffer();
            for (index, (label, value)) in
                [("y", "C"), ("m", "C"), ("S", "2"), ("V", "7"), ("D", "C")]
                    .iter()
                    .enumerate()
            {
                let y = first_row + index as u16 * 2;
                assert_eq!(buffer[(label_x, y)].symbol(), *label);
                assert_eq!(buffer[(value_x, y)].symbol(), *value);
                assert_eq!(buffer[(border_x, y)].symbol(), "\u{2502}");
            }
            assert_eq!(buffer[(border_x, popup_y + 1)].symbol(), "\u{250c}");
            assert_eq!(buffer[(border_x, popup_y + 1)].fg, ACCENT);
            assert_eq!(buffer[(value_x, first_row)].bg, Color::DarkGray);
            assert_eq!(buffer[(value_x, first_row + 2)].bg, Color::Black);
            app.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Tab,
                crossterm::event::KeyModifiers::NONE,
            ));
            terminal.draw(|frame| draw(frame, &app))?;
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer[(value_x, first_row)].bg, Color::Black);
            assert_eq!(buffer[(value_x, first_row + 2)].bg, Color::DarkGray);
            assert_eq!(buffer[(border_x, first_row + 1)].fg, ACCENT);
            app.settings_form.as_mut().unwrap().focus = 4;
            terminal.draw(|frame| draw(frame, &app))?;
            let buffer = terminal.backend().buffer();
            let last_row = first_row + 8;
            assert_eq!(buffer[(value_x, last_row)].bg, Color::DarkGray);
            assert_eq!(buffer[(border_x, last_row + 1)].symbol(), "\u{2514}");
            assert_eq!(buffer[(border_x, last_row + 1)].fg, ACCENT);
        }
        Ok(())
    }

    #[test]
    fn mute_shortcut_and_observed_mute_state_are_visible() -> anyhow::Result<()> {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut app, _directory, _commands, _events) = crate::app::tests::fixture();
        let mut terminal = Terminal::new(TestBackend::new(110, 30))?;
        for muted in [true, false] {
            app.muted = muted;
            terminal.draw(|frame| draw(frame, &app))?;
            let content = format!("{}", terminal.backend());
            assert!(!content.contains("m  Mute"));
            assert!(content.contains("Volume:   70%"));
            assert!(!content.contains("s  Stop"));
            assert!(content.contains("t  Settings"));
            assert!(!content.contains("s  Settings"));
            assert_eq!(content.contains("(muted)"), muted);
        }
        Ok(())
    }

    #[test]
    fn seek_bar_moves_playhead_and_keeps_time_on_a_separate_readable_line() -> anyhow::Result<()> {
        use ratatui::{Terminal, backend::TestBackend};

        for width in [20, 43, 75] {
            let mut terminal = Terminal::new(TestBackend::new(width, 3))?;
            for position in [0.0, 30.0, 60.0, 90.0, 120.0] {
                terminal.draw(|frame| seek_bar(frame, frame.area(), position, 120.0))?;
                let buffer = terminal.backend().buffer();
                let playhead = ((position / 120.0) * f64::from(width - 1)).round() as u16;
                for x in 0..width {
                    let cell = &buffer[(x, 1)];
                    assert_eq!(
                        cell.symbol(),
                        if x == playhead {
                            "\u{2588}"
                        } else {
                            "\u{2500}"
                        }
                    );
                    assert_eq!(
                        cell.fg,
                        if x < playhead {
                            ACCENT
                        } else if x == playhead {
                            Color::White
                        } else {
                            Color::DarkGray
                        }
                    );
                }
                let label = format!("{} / 2:00", time(position));
                let label_x = width / 2 - label.len() as u16 / 2;
                for (offset, character) in label.chars().enumerate() {
                    let cell = &buffer[(label_x + offset as u16, 2)];
                    assert_eq!(cell.symbol(), character.to_string());
                    assert_eq!(cell.fg, Color::Gray);
                    assert_eq!(cell.bg, Color::Reset);
                    assert!(!cell.modifier.contains(Modifier::REVERSED));
                }
            }
        }
        let mut terminal = Terminal::new(TestBackend::new(43, 3))?;
        terminal.draw(|frame| seek_bar(frame, frame.area(), 15.0, 0.0))?;
        assert_eq!(terminal.backend().buffer()[(0, 1)].symbol(), "\u{2588}");
        assert!(format!("{}", terminal.backend()).contains("0:15 / LIVE / unknown"));
        Ok(())
    }

    #[test]
    fn formats_elapsed_time() {
        assert_eq!(time(0.0), "0:00");
        assert_eq!(time(125.9), "2:05");
        assert_eq!(time(3661.0), "1:01:01");
        assert_eq!(time(-1.0), "0:00");
    }
}
