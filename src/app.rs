use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
};
use ratatui::{
    Terminal,
    widgets::ListState,
};
use std::time::Duration;

use crate::spotify::{CurrentlyPlaying, Playlist, Queue, SpotifyClient, Track};
use crate::ui;

#[derive(Debug, Clone, Copy)]
pub enum FocusedPane {
    Playlists,
    Tracks,
    SearchInput,
}

#[derive(Debug, Clone)]
pub enum AppState {
    Authenticating,
    Loading,
    Ready,
    Error(String),
}

pub struct App {
    pub spotify_client: SpotifyClient,
    pub playlists: Vec<Playlist>,
    pub current_tracks: Vec<Track>,
    pub search_results: Vec<Track>,
    pub currently_playing: Option<CurrentlyPlaying>,
    pub queue: Option<Queue>,
    pub playlists_state: ListState,
    pub tracks_state: ListState,
    pub search_state: ListState,
    pub focused_pane: FocusedPane,
    pub show_search: bool,
    pub search_input: String,
    pub show_playback_controls: bool,
    pub playback_controls_state: ListState,
    pub show_help: bool,
    pub state: AppState,
    pub should_quit: bool,
    pub last_search_time: Option<std::time::Instant>,
    pub search_debounce_ms: u64,
}

impl App {
    pub async fn new() -> Result<Self> {
        let client_id = std::env::var("SPOTIFY_CLIENT_ID")
            .expect("SPOTIFY_CLIENT_ID environment variable not set");
        let client_secret = std::env::var("SPOTIFY_CLIENT_SECRET")
            .expect("SPOTIFY_CLIENT_SECRET environment variable not set");

        let spotify_client = SpotifyClient::new(client_id, client_secret);

        let mut app = Self {
            spotify_client,
            playlists: Vec::new(),
            current_tracks: Vec::new(),
            search_results: Vec::new(),
            currently_playing: None,
            queue: None,
            playlists_state: ListState::default(),
            tracks_state: ListState::default(),
            search_state: ListState::default(),
            focused_pane: FocusedPane::Playlists,
            show_search: false,
            search_input: String::new(),
            show_playback_controls: false,
            playback_controls_state: ListState::default(),
            show_help: false,
            state: AppState::Authenticating,
            should_quit: false,
            last_search_time: None,
            search_debounce_ms: 500, // 300ms debounce
        };

        app.playlists_state.select(Some(0));
        app.tracks_state.select(Some(0));
        app.search_state.select(Some(0));
        app.playback_controls_state.select(Some(0));

        Ok(app)
    }

    pub async fn run(&mut self, terminal: &mut Terminal<impl ratatui::backend::Backend>) -> Result<()> {
        self.authenticate().await?;
        self.load_playlists().await?;

        let mut last_update = std::time::Instant::now();
        let mut last_refreshed = std::time::Instant::now();

        loop {
            terminal.draw(|f| ui::draw(f, self))?;

            if self.should_quit {
                break;
            }

            // Update currently playing and queue every 2 seconds
            if last_update.elapsed() >= Duration::from_secs(2) {
                self.update_currently_playing().await;
                self.update_queue().await;
                last_update = std::time::Instant::now();
            }

            // Update the refresh token every 10 mins
            if last_refreshed.elapsed() >= Duration::from_secs(600) {
                self.refresh_access_token().await?;
                last_refreshed = std::time::Instant::now();
            }

            // Check for pending search
            self.check_pending_search().await;

            if crossterm::event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    self.handle_key_event(key).await?;
                }
            }
        }

        Ok(())
    }

    async fn authenticate(&mut self) -> Result<()> {
        self.state = AppState::Authenticating;
        match self.spotify_client.authenticate().await {
            Ok(_) => {
                self.state = AppState::Ready;
                Ok(())
            }
            Err(e) => {
                self.state = AppState::Error(format!("Authentication failed: {}", e));
                Err(e)
            }
        }
    }

    async fn refresh_access_token(&mut self) -> Result<()> {
        match self.spotify_client.refresh_access_token().await {
            Ok(_) => {
                self.state = AppState::Ready;
                Ok(())
            }
            Err(e) => {
                self.state = AppState::Error(format!("Authentication failed: {}", e));
                Err(e)
            }
        }
    }

    async fn load_playlists(&mut self) -> Result<()> {
        self.state = AppState::Loading;
        match self.spotify_client.get_playlists().await {
            Ok(playlists) => {
                self.playlists = playlists;
                if !self.playlists.is_empty() {
                    self.load_playlist_tracks(0).await?;
                }
                self.state = AppState::Ready;
                Ok(())
            }
            Err(e) => {
                self.state = AppState::Error(format!("Failed to load playlists: {}", e));
                Err(e)
            }
        }
    }

    async fn load_playlist_tracks(&mut self, playlist_index: usize) -> Result<()> {
        if playlist_index < self.playlists.len() {
            let playlist_id = &self.playlists[playlist_index].id;
            self.current_tracks = self.spotify_client.get_playlist_tracks(playlist_id).await?;
            self.tracks_state.select(Some(0));
        }
        Ok(())
    }

    async fn handle_key_event(&mut self, key: KeyEvent) -> Result<()> {
        // Handle error state - any key dismisses the error
        if matches!(self.state, AppState::Error(_)) {
            self.state = AppState::Ready;
            return Ok(());
        }

        if self.show_help {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                self.show_help = false;
            }
            return Ok(());
        } else if self.show_playback_controls {
            return self.handle_playback_controls_key(key).await;
        } else if self.show_search {
            match key.code {
                KeyCode::Esc => {
                    self.show_search = false;
                    self.search_input.clear();
                    self.search_results.clear();
                    self.focused_pane = FocusedPane::Playlists;
                    self.last_search_time = None;
                }
                KeyCode::Enter => {
                    // Enter while in search mode should focus the tracks pane
                    if !self.search_results.is_empty() {
                        self.focused_pane = FocusedPane::Tracks;
                    }
                }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Ctrl+P - Previous (same as Up)
                    if matches!(self.focused_pane, FocusedPane::Tracks) && !self.search_results.is_empty() {
                        let selected = self.search_state.selected().unwrap_or(0);
                        if selected > 0 {
                            self.search_state.select(Some(selected - 1));
                        }
                    }
                }
                KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Ctrl+N - Next (same as Down)
                    if matches!(self.focused_pane, FocusedPane::Tracks) && !self.search_results.is_empty() {
                        let selected = self.search_state.selected().unwrap_or(0);
                        if selected < self.search_results.len() - 1 {
                            self.search_state.select(Some(selected + 1));
                        }
                    }
                }
                KeyCode::Char('+') => {
                    if matches!(self.focused_pane, FocusedPane::Tracks) {
                        if let Err(e) = self.add_current_track_to_queue().await {
                            self.state = AppState::Error(e.to_string());
                        }
                    }
                }
                KeyCode::Char(c) => {
                    if matches!(self.focused_pane, FocusedPane::SearchInput) {
                        self.search_input.push(c);
                        // Start debounce timer
                        self.last_search_time = Some(std::time::Instant::now());
                    }
                }
                KeyCode::Backspace => {
                    if matches!(self.focused_pane, FocusedPane::SearchInput) {
                        self.search_input.pop();
                        if self.search_input.is_empty() {
                            // Clear results immediately if search input is empty
                            self.search_results.clear();
                            self.last_search_time = None;
                        } else {
                            // Start debounce timer
                            self.last_search_time = Some(std::time::Instant::now());
                        }
                    }
                }
                KeyCode::Up => {
                    if matches!(self.focused_pane, FocusedPane::Tracks) && !self.search_results.is_empty() {
                        let selected = self.search_state.selected().unwrap_or(0);
                        if selected > 0 {
                            self.search_state.select(Some(selected - 1));
                        }
                    }
                }
                KeyCode::Down => {
                    if matches!(self.focused_pane, FocusedPane::Tracks) && !self.search_results.is_empty() {
                        let selected = self.search_state.selected().unwrap_or(0);
                        if selected < self.search_results.len() - 1 {
                            self.search_state.select(Some(selected + 1));
                        }
                    }
                }
                _ => {}
            }
        } else {
            match key.code {
                KeyCode::Char('q') => {
                    self.should_quit = true;
                }
                KeyCode::Char('s') => {
                    self.show_search = true;
                    self.search_input.clear();
                    self.search_results.clear();
                    self.focused_pane = FocusedPane::SearchInput;
                }
                KeyCode::Char(' ') => {
                    self.show_playback_controls = true;
                    self.playback_controls_state.select(Some(0));
                }
                KeyCode::Char('?') => {
                    self.show_help = true;
                }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Ctrl+P - Previous (same as Up)
                    match self.focused_pane {
                        FocusedPane::Playlists => {
                            if !self.playlists.is_empty() {
                                let selected = self.playlists_state.selected().unwrap_or(0);
                                if selected > 0 {
                                    self.playlists_state.select(Some(selected - 1));
                                    self.load_playlist_tracks(selected - 1).await?;
                                }
                            }
                        }
                        FocusedPane::Tracks => {
                            if self.show_search && !self.search_results.is_empty() {
                                if let Some(selected) = self.search_state.selected() {
                                    if selected > 0 {
                                        self.search_state.select(Some(selected - 1));
                                    }
                                }
                            } else if !self.current_tracks.is_empty() {
                                let selected = self.tracks_state.selected().unwrap_or(0);
                                if selected > 0 {
                                    self.tracks_state.select(Some(selected - 1));
                                }
                            }
                        }
                        FocusedPane::SearchInput => {
                            // No action for search input pane
                        }
                    }
                }
                KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Ctrl+N - Next (same as Down)
                    match self.focused_pane {
                        FocusedPane::Playlists => {
                            if !self.playlists.is_empty() {
                                let selected = self.playlists_state.selected().unwrap_or(0);
                                if selected < self.playlists.len() - 1 {
                                    self.playlists_state.select(Some(selected + 1));
                                    self.load_playlist_tracks(selected + 1).await?;
                                }
                            }
                        }
                        FocusedPane::Tracks => {
                            if self.show_search && !self.search_results.is_empty() {
                                if let Some(selected) = self.search_state.selected() {
                                    if selected < self.search_results.len() - 1 {
                                        self.search_state.select(Some(selected + 1));
                                    }
                                }
                            } else if !self.current_tracks.is_empty() {
                                let selected = self.tracks_state.selected().unwrap_or(0);
                                if selected < self.current_tracks.len() - 1 {
                                    self.tracks_state.select(Some(selected + 1));
                                }
                            }
                        }
                        FocusedPane::SearchInput => {
                            // No action for search input pane
                        }
                    }
                }
                KeyCode::Tab => {
                    self.focused_pane = match self.focused_pane {
                        FocusedPane::Playlists => FocusedPane::Tracks,
                        FocusedPane::Tracks => if self.show_search { FocusedPane::SearchInput } else { FocusedPane::Playlists },
                        FocusedPane::SearchInput => FocusedPane::Playlists,
                    };
                }
                KeyCode::Up => {
                    match self.focused_pane {
                        FocusedPane::Playlists => {
                            if !self.playlists.is_empty() {
                                let selected = self.playlists_state.selected().unwrap_or(0);
                                if selected > 0 {
                                    self.playlists_state.select(Some(selected - 1));
                                    self.load_playlist_tracks(selected - 1).await?;
                                }
                            }
                        }
                        FocusedPane::Tracks => {
                            if self.show_search && !self.search_results.is_empty() {
                                if let Some(selected) = self.search_state.selected() {
                                    if selected > 0 {
                                        self.search_state.select(Some(selected - 1));
                                    }
                                }
                            } else if !self.current_tracks.is_empty() {
                                let selected = self.tracks_state.selected().unwrap_or(0);
                                if selected > 0 {
                                    self.tracks_state.select(Some(selected - 1));
                                }
                            }
                        }
                        FocusedPane::SearchInput => {
                            // No action for search input pane
                        }
                    }
                }
                KeyCode::Down => {
                    match self.focused_pane {
                        FocusedPane::Playlists => {
                            if !self.playlists.is_empty() {
                                let selected = self.playlists_state.selected().unwrap_or(0);
                                if selected < self.playlists.len() - 1 {
                                    self.playlists_state.select(Some(selected + 1));
                                    self.load_playlist_tracks(selected + 1).await?;
                                }
                            }
                        }
                        FocusedPane::Tracks => {
                            if self.show_search && !self.search_results.is_empty() {
                                if let Some(selected) = self.search_state.selected() {
                                    if selected < self.search_results.len() - 1 {
                                        self.search_state.select(Some(selected + 1));
                                    }
                                }
                            } else if !self.current_tracks.is_empty() {
                                let selected = self.tracks_state.selected().unwrap_or(0);
                                if selected < self.current_tracks.len() - 1 {
                                    self.tracks_state.select(Some(selected + 1));
                                }
                            }
                        }
                        FocusedPane::SearchInput => {
                            // No action for search input pane
                        }
                    }
                }
                KeyCode::Enter => {
                    match self.focused_pane {
                        FocusedPane::Tracks => {
                            if self.show_search {
                                if let Some(selected) = self.search_state.selected() {
                                    if selected < self.search_results.len() {
                                        let track = &self.search_results[selected];
                                        if let Err(e) = self.spotify_client.play_track(&track.uri).await {
                                            self.state = AppState::Error(e.to_string());
                                        }
                                    }
                                }
                            } else if let Some(selected) = self.tracks_state.selected() {
                                if selected < self.current_tracks.len() {
                                    let track = &self.current_tracks[selected];
                                    if let Err(e) = self.spotify_client.play_track(&track.uri).await {
                                        self.state = AppState::Error(e.to_string());
                                    }
                                }
                            }
                        }
                        FocusedPane::SearchInput => {
                            // Enter in search input focuses tracks pane
                            if !self.search_results.is_empty() {
                                self.focused_pane = FocusedPane::Tracks;
                                // Select first result when focusing tracks pane
                                self.search_state.select(Some(0));
                            }
                        }
                        _ => {}
                    }
                }
                KeyCode::Char('+') => {
                    if matches!(self.focused_pane, FocusedPane::Tracks) {
                        if let Err(e) = self.add_current_track_to_queue().await {
                            self.state = AppState::Error(e.to_string());
                        }
                    }
                }
                _ => {}
            }
        }

        if self.show_search && matches!(self.focused_pane, FocusedPane::Tracks) {
            if key.code == KeyCode::Enter {
                if let Some(selected) = self.search_state.selected() {
                    if selected < self.search_results.len() {
                        let track = &self.search_results[selected];
                        if let Err(e) = self.spotify_client.play_track(&track.uri).await {
                            self.state = AppState::Error(e.to_string());
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub fn get_display_tracks(&self) -> &Vec<Track> {
        if self.show_search {
            &self.search_results
        } else {
            &self.current_tracks
        }
    }

    async fn update_currently_playing(&mut self) {
        if let Ok(currently_playing) = self.spotify_client.get_currently_playing().await {
            self.currently_playing = currently_playing;
        }
    }

    async fn update_queue(&mut self) {
        if let Ok(queue) = self.spotify_client.get_queue().await {
            self.queue = queue;
        }
    }

    async fn check_pending_search(&mut self) {
        if let Some(last_search_time) = self.last_search_time {
            if last_search_time.elapsed() >= Duration::from_millis(self.search_debounce_ms) {
                self.last_search_time = None;
                if !self.search_input.is_empty() {
                    if let Ok(results) = self.spotify_client.search_tracks(&self.search_input).await {
                        self.search_results = results;
                        // Don't auto-select first result, let user navigate first
                        self.search_state.select(None);
                    }
                }
            }
        }
    }


    async fn handle_playback_controls_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.show_playback_controls = false;
            }
            KeyCode::Up => {
                let selected = self.playback_controls_state.selected().unwrap_or(0);
                if selected > 0 {
                    self.playback_controls_state.select(Some(selected - 1));
                }
            }
            KeyCode::Down => {
                let selected = self.playback_controls_state.selected().unwrap_or(0);
                if selected < 3 { // 0: Play/Pause, 1: Previous, 2: Next, 3: Close
                    self.playback_controls_state.select(Some(selected + 1));
                }
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+P - Previous (same as Up)
                let selected = self.playback_controls_state.selected().unwrap_or(0);
                if selected > 0 {
                    self.playback_controls_state.select(Some(selected - 1));
                }
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+N - Next (same as Down)
                let selected = self.playback_controls_state.selected().unwrap_or(0);
                if selected < 3 { // 0: Play/Pause, 1: Previous, 2: Next, 3: Close
                    self.playback_controls_state.select(Some(selected + 1));
                }
            }
            KeyCode::Enter => {
                if let Some(selected) = self.playback_controls_state.selected() {
                    match selected {
                        0 => {
                            // Play/Pause
                            if let Some(ref currently_playing) = self.currently_playing {
                                if currently_playing.is_playing {
                                    if let Err(e) = self.spotify_client.pause_playback().await {
                                        self.state = AppState::Error(e.to_string());
                                    }
                                } else {
                                    if let Err(e) = self.spotify_client.resume_playback().await {
                                        self.state = AppState::Error(e.to_string());
                                    }
                                }
                            } else {
                                if let Err(e) = self.spotify_client.resume_playback().await {
                                    self.state = AppState::Error(e.to_string());
                                }
                            }
                        }
                        1 => {
                            // Previous
                            if let Err(e) = self.spotify_client.previous_track().await {
                                self.state = AppState::Error(e.to_string());
                            }
                        }
                        2 => {
                            // Next
                            if let Err(e) = self.spotify_client.next_track().await {
                                self.state = AppState::Error(e.to_string());
                            }
                        }
                        3 => {
                            // Close
                            self.show_playback_controls = false;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn add_current_track_to_queue(&mut self) -> Result<()> {
        let tracks = self.get_display_tracks().clone();
        let selected_index = if self.show_search {
            self.search_state.selected()
        } else {
            self.tracks_state.selected()
        };

        if let Some(index) = selected_index {
            if index < tracks.len() {
                let track = &tracks[index];
                match self.spotify_client.add_to_queue(&track.uri).await {
                    Ok(_) => {
                        // Immediately update the queue to show the new addition
                        self.update_queue().await;
                        Ok(())
                    }
                    Err(e) => {
                        self.state = AppState::Error(e.to_string());
                        Err(e)
                    }
                }
            } else {
                Ok(())
            }
        } else {
            Ok(())
        }
    }
}
