use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};
use std::collections::HashSet;

use crate::app::{App, AppState, FocusedPane};

pub fn draw(f: &mut Frame, app: &mut App) {
    let main_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)].as_ref())
        .split(f.size());

    let content_area = main_layout[0];
    let help_area = main_layout[1];

    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)].as_ref())
        .split(content_area);

    // Split the left side into playlists (top), currently playing (middle), and queue (bottom)
    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Percentage(50),
                Constraint::Percentage(25),
                Constraint::Percentage(25),
            ]
            .as_ref(),
        )
        .split(main_chunks[0]);

    draw_playlists(f, app, left_chunks[0]);
    draw_currently_playing(f, app, left_chunks[1]);
    draw_queue(f, app, left_chunks[2]);

    // Split the right side for search functionality
    if app.show_search {
        let right_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)].as_ref())
            .split(main_chunks[1]);

        draw_search_bar(f, app, right_chunks[0]);
        draw_tracks(f, app, right_chunks[1]);
    } else {
        draw_tracks(f, app, main_chunks[1]);
    }

    draw_help_hint(f, help_area);

    if app.show_playback_controls {
        draw_playback_controls_popup(f, app);
    }

    if app.show_help {
        draw_help_popup(f, app);
    }

    // Show error messages or status
    if let AppState::Error(ref error) = app.state {
        draw_error_popup(f, error);
    } else if matches!(app.state, AppState::Loading) {
        draw_status_popup(f, "Loading...");
    } else if matches!(app.state, AppState::Authenticating) {
        draw_status_popup(f, "Authenticating...");
    }
}

fn draw_playlists(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<ListItem> = app
        .playlists
        .iter()
        .map(|playlist| {
            let content = vec![Line::from(Span::raw(&playlist.name))];
            ListItem::new(content)
        })
        .collect();

    let border_style = if matches!(app.focused_pane, FocusedPane::Playlists) {
        Style::default().fg(Color::Green)
    } else {
        Style::default()
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Playlists")
                .border_style(border_style),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol(">> ");

    f.render_stateful_widget(list, area, &mut app.playlists_state);
}

fn draw_currently_playing(f: &mut Frame, app: &App, area: Rect) {
    let content = if let Some(ref currently_playing) = app.currently_playing {
        if let Some(ref track) = currently_playing.item {
            let artists = track
                .artists
                .iter()
                .map(|a| a.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            let device_name = currently_playing
                .device
                .as_ref()
                .map(|d| d.name.clone())
                .unwrap_or_else(|| "Unknown Device".to_string());
            let status = if currently_playing.is_playing {
                "▶"
            } else {
                "⏸"
            };

            let progress = if let Some(progress_ms) = currently_playing.progress_ms {
                let progress_sec = progress_ms / 1000;
                let progress_min = progress_sec / 60;
                let progress_sec = progress_sec % 60;
                let duration_sec = track.duration_ms / 1000;
                let duration_min = duration_sec / 60;
                let duration_sec = duration_sec % 60;
                format!(
                    " {}:{:02} / {}:{:02}",
                    progress_min, progress_sec, duration_min, duration_sec
                )
            } else {
                String::new()
            };

            vec![
                Line::from(vec![
                    Span::styled(
                        status,
                        Style::default().fg(if currently_playing.is_playing {
                            Color::Green
                        } else {
                            Color::Yellow
                        }),
                    ),
                    Span::raw(" "),
                    Span::styled(&track.name, Style::default().fg(Color::White)),
                ]),
                Line::from(Span::styled(artists, Style::default().fg(Color::Gray))),
                Line::from(Span::styled(device_name, Style::default().fg(Color::Cyan))),
                Line::from(Span::styled(progress, Style::default().fg(Color::Gray))),
            ]
        } else {
            vec![Line::from(Span::raw("No track information available"))]
        }
    } else {
        vec![Line::from(Span::raw("Nothing currently playing"))]
    };

    let paragraph = Paragraph::new(content)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Now Playing")
                .border_style(Style::default()),
        )
        .wrap(Wrap { trim: true });

    f.render_widget(paragraph, area);
}

fn draw_queue(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = if let Some(ref queue) = app.queue {
        // Filter out tracks that match the currently playing song and remove duplicates
        let currently_playing_id = queue.currently_playing.as_ref().map(|t| &t.id);
        let mut actual_queue: Vec<&crate::spotify::Track> = Vec::new();
        let mut seen_ids = HashSet::new();

        for track in &queue.queue {
            // Skip if it's the currently playing song
            if Some(&track.id) == currently_playing_id {
                continue;
            }

            // Skip if we've already seen this track (remove duplicates)
            if seen_ids.contains(&track.id) {
                continue;
            }

            seen_ids.insert(&track.id);
            actual_queue.push(track);
        }

        if actual_queue.is_empty() {
            vec![ListItem::new(vec![Line::from(Span::styled(
                "Queue is empty",
                Style::default().fg(Color::DarkGray),
            ))])]
        } else {
            actual_queue
                .iter()
                .take(10)
                .enumerate()
                .map(|(i, track)| {
                    let artists = track
                        .artists
                        .iter()
                        .map(|a| a.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ");
                    let content = vec![Line::from(vec![
                        Span::styled(format!("{}. ", i + 1), Style::default().fg(Color::DarkGray)),
                        Span::styled(&track.name, Style::default().fg(Color::White)),
                        Span::raw(" - "),
                        Span::styled(artists, Style::default().fg(Color::Gray)),
                    ])];
                    ListItem::new(content)
                })
                .collect()
        }
    } else {
        vec![ListItem::new(vec![Line::from(Span::styled(
            "No queue data available",
            Style::default().fg(Color::DarkGray),
        ))])]
    };

    let queue_count = if let Some(ref queue) = app.queue {
        // Count actual queue items (excluding currently playing and duplicates)
        let currently_playing_id = queue.currently_playing.as_ref().map(|t| &t.id);
        let mut seen_ids = HashSet::new();
        let mut actual_queue_count = 0;

        for track in &queue.queue {
            // Skip if it's the currently playing song
            if Some(&track.id) == currently_playing_id {
                continue;
            }

            // Skip if we've already seen this track
            if seen_ids.contains(&track.id) {
                continue;
            }

            seen_ids.insert(&track.id);
            actual_queue_count += 1;
        }

        if actual_queue_count == 0 {
            "Queue (0 songs)".to_string()
        } else if actual_queue_count > 10 {
            format!("Queue ({} songs, showing first 10)", actual_queue_count)
        } else {
            format!("Queue ({} songs)", actual_queue_count)
        }
    } else {
        "Queue".to_string()
    };

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(queue_count)
            .border_style(Style::default()),
    );

    f.render_widget(list, area);
}

fn draw_tracks(f: &mut Frame, app: &mut App, area: Rect) {
    let tracks = app.get_display_tracks().clone();
    let items: Vec<ListItem> = tracks
        .iter()
        .map(|track| {
            let artists = track
                .artists
                .iter()
                .map(|a| a.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            let content = vec![Line::from(vec![
                Span::styled(&track.name, Style::default().fg(Color::White)),
                Span::raw(" - "),
                Span::styled(artists, Style::default().fg(Color::Gray)),
            ])];
            ListItem::new(content)
        })
        .collect();

    let border_style = if matches!(app.focused_pane, FocusedPane::Tracks) {
        Style::default().fg(Color::Green)
    } else {
        Style::default()
    };

    let title = if app.show_search {
        "Search Results".to_string()
    } else if let Some(selected) = app.playlists_state.selected() {
        if selected < app.playlists.len() {
            app.playlists[selected].name.clone()
        } else {
            "Tracks".to_string()
        }
    } else {
        "Tracks".to_string()
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title.as_str())
                .border_style(border_style),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol(">> ");

    let state = if app.show_search {
        &mut app.search_state
    } else {
        &mut app.tracks_state
    };

    f.render_stateful_widget(list, area, state);
}

fn draw_search_bar(f: &mut Frame, app: &App, area: Rect) {
    let border_style = if matches!(app.focused_pane, FocusedPane::SearchInput) {
        Style::default().fg(Color::Green)
    } else {
        Style::default()
    };

    let input = Paragraph::new(app.search_input.as_str())
        .style(Style::default().fg(Color::Yellow))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Search")
                .border_style(border_style),
        );

    f.render_widget(input, area);

    // Only show cursor when search input is focused
    if matches!(app.focused_pane, FocusedPane::SearchInput) {
        f.set_cursor(area.x + app.search_input.len() as u16 + 1, area.y + 1);
    }
}

fn draw_playback_controls_popup(f: &mut Frame, app: &mut App) {
    let popup_area = centered_rect(40, 8, f.size());

    f.render_widget(Clear, popup_area);

    let play_pause_text = if let Some(ref currently_playing) = app.currently_playing {
        if currently_playing.is_playing {
            "⏸ Pause"
        } else {
            "▶ Play"
        }
    } else {
        "▶ Play"
    };

    let items = vec![
        ListItem::new(Line::from(play_pause_text)),
        ListItem::new(Line::from("⏮ Previous")),
        ListItem::new(Line::from("⏭ Next")),
        ListItem::new(Line::from("✕ Close")),
    ];

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Playback Controls")
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol(">> ");

    f.render_stateful_widget(list, popup_area, &mut app.playback_controls_state);
}

fn draw_help_popup(f: &mut Frame, _app: &App) {
    let popup_area = centered_rect(80, 22, f.size());

    f.render_widget(Clear, popup_area);

    let help_text = vec![
        Line::from(vec![Span::styled(
            "Navigation",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Tab", Style::default().fg(Color::Green)),
            Span::raw("           Switch between playlists and tracks panes"),
        ]),
        Line::from(vec![
            Span::styled("↑/↓ or Ctrl+P/N", Style::default().fg(Color::Green)),
            Span::raw(" Navigate up/down in current pane"),
        ]),
        Line::from(vec![
            Span::styled("Enter", Style::default().fg(Color::Green)),
            Span::raw("         Play track or load playlist"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Features",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled("s", Style::default().fg(Color::Green)),
            Span::raw("             Search for tracks"),
        ]),
        Line::from(vec![
            Span::styled("Space", Style::default().fg(Color::Green)),
            Span::raw("         Open playback controls"),
        ]),
        Line::from(vec![
            Span::styled("+", Style::default().fg(Color::Green)),
            Span::raw("             Add track to queue"),
        ]),
        Line::from(vec![
            Span::styled("q", Style::default().fg(Color::Green)),
            Span::raw("             Quit application"),
        ]),
        Line::from(vec![
            Span::styled("?", Style::default().fg(Color::Green)),
            Span::raw("             Show this help"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Playback Controls",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from("Press Space to open playback controls popup with:"),
        Line::from("  • Play/Pause current track"),
        Line::from("  • Skip to previous/next track"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Press Esc or ? to close this help",
            Style::default().fg(Color::Cyan),
        )]),
    ];

    let paragraph = Paragraph::new(help_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Help - SpotiTUI")
                .border_style(Style::default().fg(Color::Blue)),
        )
        .wrap(Wrap { trim: true });

    f.render_widget(paragraph, popup_area);
}

fn draw_help_hint(f: &mut Frame, area: Rect) {
    let help_text = vec![Line::from(vec![
        Span::raw("Press "),
        Span::styled("?", Style::default().fg(Color::Yellow)),
        Span::raw(" for help  |  "),
        Span::styled("Tab", Style::default().fg(Color::Cyan)),
        Span::raw(" to switch panes  |  "),
        Span::styled("q", Style::default().fg(Color::Red)),
        Span::raw(" to quit  |  "),
        Span::styled("Space", Style::default().fg(Color::Green)),
        Span::raw(" for controls  |  "),
        Span::styled("s", Style::default().fg(Color::LightBlue)),
        Span::raw(" for search"),
    ])];

    let paragraph = Paragraph::new(help_text)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);

    f.render_widget(paragraph, area);
}

fn draw_error_popup(f: &mut Frame, error: &str) {
    let popup_area = centered_rect(60, 5, f.size());

    f.render_widget(Clear, popup_area);

    let error_text = Paragraph::new(error)
        .style(Style::default().fg(Color::Red))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Error - Press any key to continue"),
        );

    f.render_widget(error_text, popup_area);
}

fn draw_status_popup(f: &mut Frame, status: &str) {
    let popup_area = centered_rect(40, 3, f.size());

    f.render_widget(Clear, popup_area);

    let status_text = Paragraph::new(status)
        .style(Style::default().fg(Color::Yellow))
        .block(Block::default().borders(Borders::ALL).title("Status"));

    f.render_widget(status_text, popup_area);
}

fn centered_rect(percent_x: u16, height: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((r.height - height) / 2),
            Constraint::Length(height),
            Constraint::Length((r.height - height) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
