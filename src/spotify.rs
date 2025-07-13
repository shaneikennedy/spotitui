use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use rand::Rng;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener as AsyncTcpListener;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LikedTrackResponse {
    items: Vec<LikedTrack>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LikedTrack {
    added_at: String,
    track: Track,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub name: String,
    pub artists: Vec<Artist>,
    pub album: Album,
    pub duration_ms: u32,
    pub uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artist {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Album {
    pub id: String,
    pub name: String,
    pub images: Vec<Image>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    pub height: Option<u32>,
    pub url: String,
    pub width: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub tracks: PlaylistTracks,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistTracks {
    pub total: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: u32,
    refresh_token: Option<String>,
    scope: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct PlaylistsResponse {
    items: Vec<Playlist>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PlaylistTracksResponse {
    items: Vec<PlaylistTrackItem>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PlaylistTrackItem {
    track: Track,
}

#[derive(Debug, Serialize, Deserialize)]
struct SearchResponse {
    tracks: TracksResponse,
}

#[derive(Debug, Serialize, Deserialize)]
struct TracksResponse {
    items: Vec<Track>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: Option<String>,
    pub name: String,
    #[serde(rename = "type")]
    pub device_type: String,
    pub is_active: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct DevicesResponse {
    devices: Vec<Device>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentlyPlaying {
    pub item: Option<Track>,
    pub is_playing: bool,
    pub progress_ms: Option<u64>,
    pub device: Option<Device>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CurrentlyPlayingResponse {
    item: Option<Track>,
    is_playing: bool,
    progress_ms: Option<u64>,
    device: Option<Device>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Queue {
    pub currently_playing: Option<Track>,
    pub queue: Vec<Track>,
}

#[derive(Debug, Serialize, Deserialize)]
struct QueueResponse {
    currently_playing: Option<Track>,
    queue: Vec<Track>,
}

#[derive(Debug, Deserialize)]
struct TokenRefreshResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

pub struct SpotifyClient {
    client: Client,
    access_token: Arc<Mutex<Option<String>>>,
    refresh_token: Arc<Mutex<Option<String>>>,
    client_id: String,
}

impl SpotifyClient {
    pub fn new(client_id: String, _client_secret: String) -> Self {
        Self {
            client: Client::new(),
            access_token: Arc::new(Mutex::new(None)),
            refresh_token: Arc::new(Mutex::new(None)),
            client_id,
        }
    }

    pub async fn refresh_access_token(&self) -> Result<()> {
        let mut refresh_token = self.refresh_token.lock().await;
        let refresh_token_value = refresh_token.clone().unwrap();

        let params = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token_value.as_str()),
            ("client_id", self.client_id.as_str()),
        ];

        let response = self
            .client
            .post("https://accounts.spotify.com/api/token")
            .form(&params)
            .send()
            .await
            .context("Failed to send token refresh request")?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Token refresh failed with status {}: {}",
                status,
                error_text
            ));
        }

        let token_response: TokenRefreshResponse = response
            .json()
            .await
            .context("Failed to deserialize token response")?;

        let mut access_token = self.access_token.lock().await;
        *access_token = Some(token_response.access_token);
        *refresh_token = token_response.refresh_token;
        Ok(())
    }

    pub async fn authenticate(&self) -> Result<()> {
        let redirect_uri = "http://127.0.0.1:8888/callback";
        let scope = "user-read-private user-read-email playlist-read-private playlist-read-collaborative user-modify-playback-state user-read-playback-state user-read-currently-playing user-read-playback-position user-library-read";

        let code_verifier = self.generate_code_verifier();
        let code_challenge = self.generate_code_challenge(&code_verifier);
        let state = self.generate_state();

        let auth_url = format!(
            "https://accounts.spotify.com/authorize?client_id={}&response_type=code&redirect_uri={}&code_challenge_method=S256&code_challenge={}&state={}&scope={}",
            self.client_id,
            urlencoding::encode(redirect_uri),
            code_challenge,
            state,
            urlencoding::encode(scope)
        );

        webbrowser::open(&auth_url)?;

        let auth_code = match self.start_callback_server_with_timeout().await {
            Ok(code) => code,
            Err(_) => {
                // Fallback to manual entry - this will be handled by the UI layer
                return Err(anyhow!(
                    "Authentication callback failed - manual entry required"
                ));
            }
        };

        let token = self
            .exchange_code_for_token(&auth_code, &code_verifier, redirect_uri)
            .await?;

        let mut access_token = self.access_token.lock().await;
        *access_token = Some(token.access_token);

        let mut refresh_token = self.refresh_token.lock().await;
        *refresh_token = token.refresh_token;

        Ok(())
    }

    async fn start_callback_server_with_timeout(&self) -> Result<String> {
        timeout(Duration::from_secs(60), self.start_callback_server()).await?
    }

    async fn start_callback_server(&self) -> Result<String> {
        let listener = AsyncTcpListener::bind("127.0.0.1:8888").await?;

        loop {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    let mut buffer = vec![0; 2048];

                    // Give the client time to send the request
                    tokio::time::sleep(Duration::from_millis(100)).await;

                    match stream.try_read(&mut buffer) {
                        Ok(n) => {
                            let request = String::from_utf8_lossy(&buffer[..n]);

                            if let Some(code) = self.extract_code_from_request(&request) {
                                self.send_async_response(&mut stream).await?;
                                return Ok(code);
                            }
                        }
                        Err(_) => {
                            // Try again with a blocking read
                            let mut buffer = vec![0; 2048];
                            match stream.readable().await {
                                Ok(_) => match stream.try_read(&mut buffer) {
                                    Ok(n) => {
                                        let request = String::from_utf8_lossy(&buffer[..n]);

                                        if let Some(code) = self.extract_code_from_request(&request)
                                        {
                                            self.send_async_response(&mut stream).await?;
                                            return Ok(code);
                                        }
                                    }
                                    Err(_) => continue,
                                },
                                Err(_) => continue,
                            }
                        }
                    }
                }
                Err(_) => continue, // Don't log connection errors
            }
        }
    }

    fn extract_code_from_request(&self, request: &str) -> Option<String> {
        // Look for both /callback and / endpoints
        let patterns = ["GET /callback?", "GET /?"];

        for pattern in &patterns {
            if let Some(query_start) = request.find(pattern) {
                let query_part = &request[query_start + pattern.len()..];
                if let Some(query_end) = query_part.find(' ') {
                    let query = &query_part[..query_end];
                    let url = format!("http://127.0.0.1:8888/?{}", query);
                    if let Ok(parsed_url) = Url::parse(&url) {
                        for (key, value) in parsed_url.query_pairs() {
                            if key == "code" {
                                return Some(value.to_string());
                            }
                        }
                    }
                }
            }
        }
        None
    }

    async fn send_async_response(&self, stream: &mut tokio::net::TcpStream) -> Result<()> {
        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<html><body><h1>Authentication successful!</h1><p>You can close this window and return to the terminal.</p></body></html>";
        stream.try_write(response.as_bytes())?;
        Ok(())
    }

    async fn exchange_code_for_token(
        &self,
        code: &str,
        code_verifier: &str,
        redirect_uri: &str,
    ) -> Result<TokenResponse> {
        let mut params = HashMap::new();
        params.insert("grant_type", "authorization_code");
        params.insert("code", code);
        params.insert("redirect_uri", redirect_uri);
        params.insert("client_id", &self.client_id);
        params.insert("code_verifier", code_verifier);

        let response = self
            .client
            .post("https://accounts.spotify.com/api/token")
            .form(&params)
            .send()
            .await?;

        let token: TokenResponse = response.json().await?;
        Ok(token)
    }

    pub async fn get_playlists(&self) -> Result<Vec<Playlist>> {
        let access_token = self.access_token.lock().await;
        let token = access_token
            .as_ref()
            .ok_or_else(|| anyhow!("Not authenticated"))?;

        let response = self
            .client
            .get("https://api.spotify.com/v1/me/playlists")
            .bearer_auth(token)
            .send()
            .await
            .context("somehow in get_playlists");

        let response = response?;
        let mut playlists: PlaylistsResponse = response.json().await?;
        let liked_songs = Playlist {
            id: "liked".into(),
            name: "Liked Songs".into(),
            description: None,
            tracks: PlaylistTracks { total: 50 },
        };
        playlists.items.insert(0, liked_songs);
        Ok(playlists.items)
    }

    pub async fn get_playlist_tracks(&self, playlist_id: &str) -> Result<Vec<Track>> {
        let access_token = self.access_token.lock().await;
        let token = access_token
            .as_ref()
            .ok_or_else(|| anyhow!("Not authenticated"))?;

        let tracks: Vec<Track> = match playlist_id {
            "liked" => {
                let response = self
                    .client
                    .get("https://api.spotify.com/v1/me/tracks?limit=50")
                    .bearer_auth(token)
                    .send()
                    .await?;
                let liked_tracks_response: LikedTrackResponse =
                    response.json().await.context("it's fucking here")?;
                liked_tracks_response
                    .items
                    .into_iter()
                    .map(|item| item.track)
                    .collect()
            }
            _ => {
                let response = self
                    .client
                    .get(format!(
                        "https://api.spotify.com/v1/playlists/{}/tracks",
                        playlist_id
                    ))
                    .bearer_auth(token)
                    .send()
                    .await?;
                let tracks_response: PlaylistTracksResponse =
                    response.json().await.context("here")?;
                tracks_response
                    .items
                    .into_iter()
                    .map(|item| item.track)
                    .collect()
            }
        };

        Ok(tracks)
    }

    pub async fn search_tracks(&self, query: &str) -> Result<Vec<Track>> {
        let access_token = self.access_token.lock().await;
        let token = access_token
            .as_ref()
            .ok_or_else(|| anyhow!("Not authenticated"))?;

        let response = self
            .client
            .get("https://api.spotify.com/v1/search")
            .query(&[("q", query), ("type", "track"), ("limit", "50")])
            .bearer_auth(token)
            .send()
            .await?;

        let search_response: SearchResponse = response.json().await?;
        Ok(search_response.tracks.items)
    }

    pub async fn play_track(&self, track_uri: &str) -> Result<()> {
        let access_token = self.access_token.lock().await;
        let token = access_token
            .as_ref()
            .ok_or_else(|| anyhow!("Not authenticated"))?;

        // First, check if there are any available devices
        let devices = self.get_available_devices(token).await?;
        if devices.is_empty() {
            return Err(anyhow!("No active Spotify devices found. Please open Spotify on your phone, computer, or web browser."));
        }

        let mut body = HashMap::new();
        body.insert("uris", vec![track_uri]);

        let response = self
            .client
            .put("https://api.spotify.com/v1/me/player/play")
            .bearer_auth(token)
            .json(&body)
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            match status.as_u16() {
                404 => Err(anyhow!("No active device found. Please start Spotify on your phone, computer, or web browser.")),
                403 => Err(anyhow!("Spotify Premium is required for playback control.")),
                _ => Err(anyhow!("Failed to play track: {}", status))
            }
        }
    }

    async fn get_available_devices(&self, token: &str) -> Result<Vec<Device>> {
        let response = self
            .client
            .get("https://api.spotify.com/v1/me/player/devices")
            .bearer_auth(token)
            .send()
            .await?;

        if response.status().is_success() {
            let devices_response: DevicesResponse = response.json().await?;
            Ok(devices_response.devices)
        } else {
            Ok(Vec::new())
        }
    }

    pub async fn get_currently_playing(&self) -> Result<Option<CurrentlyPlaying>> {
        let access_token = self.access_token.lock().await;
        let token = access_token
            .as_ref()
            .ok_or_else(|| anyhow!("Not authenticated"))?;

        let response = self
            .client
            .get("https://api.spotify.com/v1/me/player/currently-playing")
            .bearer_auth(token)
            .send()
            .await?;

        if response.status().is_success() {
            let response_text = response.text().await?;
            if response_text.is_empty() {
                // No content means nothing is currently playing
                Ok(None)
            } else {
                let currently_playing_response: CurrentlyPlayingResponse =
                    serde_json::from_str(&response_text)?;
                Ok(Some(CurrentlyPlaying {
                    item: currently_playing_response.item,
                    is_playing: currently_playing_response.is_playing,
                    progress_ms: currently_playing_response.progress_ms,
                    device: currently_playing_response.device,
                }))
            }
        } else if response.status().as_u16() == 204 {
            // 204 No Content means nothing is currently playing
            Ok(None)
        } else {
            Ok(None) // Don't error on other status codes, just return None
        }
    }

    pub async fn get_queue(&self) -> Result<Option<Queue>> {
        let access_token = self.access_token.lock().await;
        let token = access_token
            .as_ref()
            .ok_or_else(|| anyhow!("Not authenticated"))?;

        let response = self
            .client
            .get("https://api.spotify.com/v1/me/player/queue")
            .bearer_auth(token)
            .send()
            .await?;

        if response.status().is_success() {
            let queue_response: QueueResponse = response.json().await?;
            Ok(Some(Queue {
                currently_playing: queue_response.currently_playing,
                queue: queue_response.queue,
            }))
        } else {
            Ok(None) // Don't error on other status codes, just return None
        }
    }

    pub async fn add_to_queue(&self, track_uri: &str) -> Result<()> {
        let access_token = self.access_token.lock().await;
        let token = access_token
            .as_ref()
            .ok_or_else(|| anyhow!("Not authenticated"))?;

        let response = self
            .client
            .post("https://api.spotify.com/v1/me/player/queue")
            .bearer_auth(token)
            .query(&[("uri", track_uri)])
            .header("Content-Length", "0")
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            match status.as_u16() {
                404 => Err(anyhow!("No active device found. Please start Spotify on your phone, computer, or web browser.")),
                403 => Err(anyhow!("Spotify Premium is required for queue control.")),
                _ => Err(anyhow!("Failed to add to queue: {}", status))
            }
        }
    }

    pub async fn pause_playback(&self) -> Result<()> {
        let access_token = self.access_token.lock().await;
        let token = access_token
            .as_ref()
            .ok_or_else(|| anyhow!("Not authenticated"))?;

        let response = self
            .client
            .put("https://api.spotify.com/v1/me/player/pause")
            .bearer_auth(token)
            .header("Content-Length", "0")
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            match status.as_u16() {
                404 => Err(anyhow!("No active device found. Please start Spotify on your phone, computer, or web browser.")),
                403 => Err(anyhow!("Spotify Premium is required for playback control.")),
                _ => Err(anyhow!("Failed to pause playback: {}", status))
            }
        }
    }

    pub async fn resume_playback(&self) -> Result<()> {
        let access_token = self.access_token.lock().await;
        let token = access_token
            .as_ref()
            .ok_or_else(|| anyhow!("Not authenticated"))?;

        let response = self
            .client
            .put("https://api.spotify.com/v1/me/player/play")
            .bearer_auth(token)
            .header("Content-Length", "0")
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            match status.as_u16() {
                404 => Err(anyhow!("No active device found. Please start Spotify on your phone, computer, or web browser.")),
                403 => Err(anyhow!("Spotify Premium is required for playback control.")),
                _ => Err(anyhow!("Failed to resume playback: {}", status))
            }
        }
    }

    pub async fn next_track(&self) -> Result<()> {
        let access_token = self.access_token.lock().await;
        let token = access_token
            .as_ref()
            .ok_or_else(|| anyhow!("Not authenticated"))?;

        let response = self
            .client
            .post("https://api.spotify.com/v1/me/player/next")
            .bearer_auth(token)
            .header("Content-Length", "0")
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            match status.as_u16() {
                404 => Err(anyhow!("No active device found. Please start Spotify on your phone, computer, or web browser.")),
                403 => Err(anyhow!("Spotify Premium is required for playback control.")),
                _ => Err(anyhow!("Failed to skip to next track: {}", status))
            }
        }
    }

    pub async fn previous_track(&self) -> Result<()> {
        let access_token = self.access_token.lock().await;
        let token = access_token
            .as_ref()
            .ok_or_else(|| anyhow!("Not authenticated"))?;

        let response = self
            .client
            .post("https://api.spotify.com/v1/me/player/previous")
            .bearer_auth(token)
            .header("Content-Length", "0")
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            match status.as_u16() {
                404 => Err(anyhow!("No active device found. Please start Spotify on your phone, computer, or web browser.")),
                403 => Err(anyhow!("Spotify Premium is required for playback control.")),
                _ => Err(anyhow!("Failed to skip to previous track: {}", status))
            }
        }
    }

    fn generate_code_verifier(&self) -> String {
        let mut rng = rand::rng();
        let code_verifier: String = (0..128)
            .map(|_| {
                let chars = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
                chars[rng.random_range(0..chars.len())] as char
            })
            .collect();
        code_verifier
    }

    fn generate_code_challenge(&self, code_verifier: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(code_verifier.as_bytes());
        let digest = hasher.finalize();
        general_purpose::URL_SAFE_NO_PAD.encode(digest)
    }

    fn generate_state(&self) -> String {
        let mut rng = rand::rng();
        (0..16)
            .map(|_| rng.random_range(0..16))
            .map(|n| format!("{:x}", n))
            .collect()
    }
}
