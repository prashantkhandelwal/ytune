use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    process::Stdio,
    sync::mpsc::TryRecvError,
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, de::DeserializeOwned};
use tokio::process::Command;

use crate::metadata::Lookup;

const YOUTUBE_RESULT_LIMIT: usize = 5;
const SETTLE_DELAY: Duration = Duration::from_millis(400);
const REQUEST_INTERVAL: Duration = Duration::from_secs(1);
const MAX_RETRIES: u32 = 2;

#[derive(Default)]
struct RequestSchedule {
    last_request: Option<Instant>,
    cooldown: Option<Instant>,
}

static REQUEST_SCHEDULE: tokio::sync::Mutex<RequestSchedule> =
    tokio::sync::Mutex::const_new(RequestSchedule {
        last_request: None,
        cooldown: None,
    });

#[derive(Default)]
pub struct RelatedSongs {
    pub artist: String,
    pub songs: Vec<String>,
    pub artist_matches: Vec<ArtistMatch>,
    pub error: Option<String>,
    pub loading: bool,
    title: String,
    cache: HashMap<String, Vec<String>>,
    lookup: Option<Lookup<MusicBrainzResult>>,
    deadline: Option<Instant>,
    selected_artist_id: Option<String>,
}

enum MusicBrainzResult {
    Artists(Vec<ArtistMatch>),
    Recordings(Vec<String>),
}

impl RelatedSongs {
    pub fn item_count(&self) -> usize {
        if self.artist_matches.is_empty() {
            self.songs.len()
        } else {
            self.artist_matches.len()
        }
    }

    pub fn choose_artist(&mut self, index: usize) {
        let Some(artist) = self.artist_matches.get(index) else {
            return;
        };
        self.selected_artist_id = Some(artist.id.clone());
        self.artist_matches.clear();
        self.loading = true;
        self.error = None;
        self.deadline = Some(Instant::now() + SETTLE_DELAY);
    }

    pub fn update(&mut self, artist: &str, title: &str, now: Instant, limit: usize) {
        let artist = artist.trim();
        let changed = normalize(artist) != normalize(&self.artist);
        if changed {
            self.lookup = None;
            self.artist = artist.to_owned();
            self.songs.clear();
            self.artist_matches.clear();
            self.selected_artist_id = None;
            self.error = None;
            self.loading = !artist.is_empty();
            self.deadline = self.loading.then_some(now + SETTLE_DELAY);
        }
        self.title = title.to_owned();
        if artist.is_empty() {
            return;
        }
        let key = normalize(artist);
        if let Some(titles) = self.cache.get(&key) {
            self.songs = other_songs(titles, title, limit);
            self.loading = false;
            self.deadline = None;
            return;
        }
        if self.lookup.is_none() && self.deadline.is_some_and(|deadline| now >= deadline) {
            let artist = self.artist.clone();
            let artist_id = self.selected_artist_id.take();
            self.lookup = Some(Lookup::spawn("MusicBrainz", move || {
                recordings(artist, artist_id)
            }));
            self.deadline = None;
        }
        if let Some(lookup) = &self.lookup {
            let result = match lookup.results.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Err("MusicBrainz lookup stopped unexpectedly".into()))
                }
            };
            if let Some(result) = result {
                self.lookup = None;
                self.loading = false;
                match result {
                    Ok(MusicBrainzResult::Recordings(titles)) => {
                        self.songs = other_songs(&titles, &self.title, limit);
                        self.cache.insert(key, titles);
                    }
                    Ok(MusicBrainzResult::Artists(artists)) => self.artist_matches = artists,
                    Err(error) => self.error = Some(error),
                }
            }
        }
    }

    pub fn retry(&mut self) {
        self.cache.remove(&normalize(&self.artist));
        self.lookup = None;
        self.artist_matches.clear();
        self.selected_artist_id = None;
        self.error = None;
        self.loading = !self.artist.is_empty();
        self.deadline = self.loading.then_some(Instant::now() + SETTLE_DELAY);
    }
}

fn normalize(value: &str) -> String {
    value.trim().to_lowercase()
}

fn other_songs(titles: &[String], current: &str, limit: usize) -> Vec<String> {
    let current = normalize(current);
    titles
        .iter()
        .filter(|title| normalize(title) != current)
        .take(limit)
        .cloned()
        .collect()
}

fn artist_query(artist: &str) -> String {
    let mut escaped = String::new();
    for character in artist.chars() {
        if r#"+-&|!(){}[]^"~*?:\/"#.contains(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    format!("artist:\"{escaped}\"")
}

#[derive(Deserialize)]
struct RecordingResponse {
    recordings: Vec<Recording>,
}

#[derive(Deserialize)]
struct Recording {
    title: String,
    #[serde(rename = "artist-credit")]
    artist_credit: Vec<ArtistCredit>,
}

#[derive(Deserialize)]
struct ArtistCredit {
    artist: ArtistMatch,
}

#[derive(Deserialize)]
pub struct ArtistMatch {
    pub id: String,
    pub name: String,
    disambiguation: Option<String>,
    country: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

impl ArtistMatch {
    pub fn label(&self) -> String {
        if let Some(description) = self
            .disambiguation
            .as_deref()
            .filter(|text| !text.is_empty())
        {
            format!("{} - {description}", self.name)
        } else {
            format!(
                "{} - {} {} [{}]",
                self.name,
                self.kind.as_deref().unwrap_or("Artist"),
                self.country.as_deref().unwrap_or(""),
                self.id.chars().take(8).collect::<String>()
            )
        }
    }
}

#[derive(Deserialize)]
struct ArtistResponse {
    artists: Vec<ArtistMatch>,
}

fn artist_candidates(response: ArtistResponse, name: &str) -> Vec<ArtistMatch> {
    let name_key = normalize(name);
    response
        .artists
        .into_iter()
        .filter(|artist| normalize(&artist.name) == name_key)
        .collect()
}

fn recording_titles(response: RecordingResponse, artist_id: &str) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut titles = Vec::new();
    for recording in response.recordings {
        if !recording
            .artist_credit
            .iter()
            .any(|credit| credit.artist.id == artist_id)
        {
            continue;
        }
        let title = recording.title.trim();
        if title.is_empty() {
            bail!("MusicBrainz returned an empty recording title");
        }
        if seen.insert(normalize(title)) {
            titles.push(title.to_owned());
        }
    }
    Ok(titles)
}

async fn recordings(artist: String, artist_id: Option<String>) -> Result<MusicBrainzResult> {
    let client = reqwest::Client::builder()
        .user_agent(concat!(
            "ytune/",
            env!("CARGO_PKG_VERSION"),
            " (https://github.com/prashantkhandelwal/ytune)"
        ))
        .timeout(Duration::from_secs(20))
        .build()
        .context("Cannot create MusicBrainz client")?;
    let artist_id = if let Some(id) = artist_id {
        id
    } else {
        let response = musicbrainz_get(&client, "artist", artist_query(&artist)).await?;
        let mut artists = artist_candidates(response, &artist);
        if artists.len() > 1 {
            return Ok(MusicBrainzResult::Artists(artists));
        }
        let Some(artist) = artists.pop() else {
            return Ok(MusicBrainzResult::Recordings(Vec::new()));
        };
        artist.id
    };
    let response = musicbrainz_get(&client, "recording", format!("arid:\"{artist_id}\"")).await?;
    Ok(MusicBrainzResult::Recordings(recording_titles(
        response, &artist_id,
    )?))
}

async fn musicbrainz_get<T: DeserializeOwned>(
    client: &reqwest::Client,
    resource: &str,
    query: String,
) -> Result<T> {
    let request = client
        .get(format!("https://musicbrainz.org/ws/2/{resource}/"))
        .query(&[
            ("query", query),
            ("fmt", "json".to_owned()),
            (
                "limit",
                if resource == "artist" { "25" } else { "100" }.to_owned(),
            ),
        ]);
    musicbrainz_response(request)
        .await?
        .json()
        .await
        .context("Invalid MusicBrainz response; press f to retry")
}

async fn musicbrainz_response(request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
    let mut retries = 0;
    loop {
        let mut schedule = REQUEST_SCHEDULE.lock().await;
        wait_for_request(&mut schedule).await;
        let response = request
            .try_clone()
            .context("Cannot recreate MusicBrainz request")?
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Cannot reach MusicBrainz; press f to retry")?;
        let status = response.status();
        if matches!(status.as_u16(), 429 | 502 | 503 | 504) {
            let delay = retry_delay(response.headers(), retries, SystemTime::now())?;
            schedule.cooldown = Some(
                Instant::now()
                    .checked_add(delay)
                    .context("MusicBrainz returned an invalid Retry-After delay")?,
            );
            if retries == MAX_RETRIES {
                bail!(
                    "MusicBrainz is temporarily unavailable (HTTP {}) after {} attempts. Press f to retry.",
                    status.as_u16(),
                    MAX_RETRIES + 1
                );
            }
            retries += 1;
            continue;
        }
        return response
            .error_for_status()
            .map_err(reqwest::Error::without_url)
            .context("MusicBrainz request failed; press f to retry");
    }
}

fn retry_delay(
    headers: &reqwest::header::HeaderMap,
    retries: u32,
    now: SystemTime,
) -> Result<Duration> {
    let Some(value) = headers.get(reqwest::header::RETRY_AFTER) else {
        return Ok(Duration::from_secs(2 << retries));
    };
    let value = value
        .to_str()
        .context("Invalid MusicBrainz Retry-After header")?;
    if let Ok(seconds) = value.parse::<u64>() {
        return Ok(Duration::from_secs(seconds));
    }
    let date = httpdate::parse_http_date(value).context("Invalid MusicBrainz Retry-After date")?;
    Ok(date.duration_since(now).unwrap_or(Duration::ZERO))
}

async fn wait_for_request(schedule: &mut RequestSchedule) {
    let deadline = schedule
        .last_request
        .map(|last| last + REQUEST_INTERVAL)
        .into_iter()
        .chain(schedule.cooldown)
        .max();
    if let Some(deadline) = deadline {
        tokio::time::sleep(deadline.saturating_duration_since(Instant::now())).await;
    }
    schedule.last_request = Some(Instant::now());
    schedule.cooldown = None;
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub uploader: String,
}

pub struct YoutubeSearch {
    pub query: String,
    pub category: String,
    pub results: Vec<SearchResult>,
    pub selected: usize,
    pub error: Option<String>,
    lookup: Option<Lookup<Vec<SearchResult>>>,
}

impl YoutubeSearch {
    #[cfg(test)]
    pub fn completed(category: String, result: Result<Vec<SearchResult>, String>) -> Self {
        let mut search = Self {
            query: "Artist Song".into(),
            category,
            results: Vec::new(),
            selected: 0,
            error: None,
            lookup: Some(Lookup::completed(result)),
        };
        search.poll();
        search
    }

    pub fn start(query: String, category: String, executable: OsString) -> Self {
        let search = query.clone();
        Self {
            query,
            category,
            results: Vec::new(),
            selected: 0,
            error: None,
            lookup: Some(Lookup::spawn("YouTube search", move || {
                search_youtube(search, executable)
            })),
        }
    }

    pub fn loading(&self) -> bool {
        self.lookup.is_some()
    }

    pub fn poll(&mut self) {
        let Some(lookup) = &self.lookup else {
            return;
        };
        let result = match lookup.results.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("YouTube search stopped unexpectedly".into()),
        };
        self.lookup = None;
        match result {
            Ok(results) => self.results = results,
            Err(error) => self.error = Some(error),
        }
    }
}

async fn search_youtube(query: String, executable: OsString) -> Result<Vec<SearchResult>> {
    let output = Command::new(executable)
        .args([
            "--flat-playlist",
            "--dump-single-json",
            "--skip-download",
            "--no-progress",
            "--socket-timeout",
            "15",
            "--retries",
            "1",
            "--extractor-retries",
            "1",
            "--",
            &format!("ytsearch{YOUTUBE_RESULT_LIMIT}:{query}"),
        ])
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .context("Cannot run yt-dlp for YouTube search; check its path in Settings")?;
    if !output.status.success() {
        bail!(
            "YouTube search failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    parse_search(&output.stdout)
}

fn parse_search(json: &[u8]) -> Result<Vec<SearchResult>> {
    #[derive(Deserialize)]
    struct Response {
        entries: Vec<Option<Entry>>,
    }
    #[derive(Deserialize)]
    struct Entry {
        id: String,
        title: String,
        channel: Option<String>,
        uploader: Option<String>,
    }
    let response: Response =
        serde_json::from_slice(json).context("Invalid yt-dlp search response")?;
    let mut results = Vec::new();
    let mut seen = HashSet::new();
    for entry in response.entries.into_iter().flatten() {
        if entry.id.len() != 11
            || !entry
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || entry.title.trim().is_empty()
        {
            bail!("YouTube search returned an invalid video ID or title");
        }
        if seen.insert(entry.id.clone()) && results.len() < YOUTUBE_RESULT_LIMIT {
            results.push(SearchResult {
                title: entry.title,
                url: format!("https://www.youtube.com/watch?v={}", entry.id),
                uploader: entry.channel.or(entry.uploader).unwrap_or_default(),
            });
        }
    }
    Ok(results)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    async fn mock_musicbrainz(statuses: Vec<u16>) -> Result<reqwest::Response> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            for status in statuses {
                let (mut stream, _) = listener.accept().await?;
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                loop {
                    line.clear();
                    if reader.read_line(&mut line).await? == 0 {
                        bail!("Mock MusicBrainz connection closed before the request headers");
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 {status} Test\r\nContent-Length: 2\r\nRetry-After: 0\r\nConnection: close\r\n\r\n{{}}"
                        )
                        .as_bytes(),
                    )
                    .await?;
            }
            Ok::<_, anyhow::Error>(())
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()?;
        let response = musicbrainz_response(client.get(format!("http://{address}/"))).await;
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .context("Mock MusicBrainz server did not receive the expected attempts")???;
        response
    }

    #[test]
    fn transient_musicbrainz_failures_retry_recover_and_stop_after_three_attempts() -> Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let response = mock_musicbrainz(vec![503, 429, 200]).await?;
            assert!(response.status().is_success());
            let error = mock_musicbrainz(vec![503, 503, 503])
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("503"));
            assert!(error.contains("3 attempts"));
            assert!(error.contains("Press f"));
            assert!(!error.contains("http://"));
            let error = mock_musicbrainz(vec![400]).await.unwrap_err().to_string();
            assert!(error.contains("press f"));
            Ok(())
        })
    }

    #[test]
    fn retry_delays_use_backoff_and_both_retry_after_formats() -> Result<()> {
        use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};

        let mut headers = HeaderMap::new();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert_eq!(retry_delay(&headers, 0, now)?, Duration::from_secs(2));
        assert_eq!(retry_delay(&headers, 1, now)?, Duration::from_secs(4));
        headers.insert(RETRY_AFTER, HeaderValue::from_static("7"));
        assert_eq!(retry_delay(&headers, 0, now)?, Duration::from_secs(7));
        headers.insert(
            RETRY_AFTER,
            HeaderValue::from_str(&httpdate::fmt_http_date(now + Duration::from_secs(12)))?,
        );
        assert_eq!(retry_delay(&headers, 0, now)?, Duration::from_secs(12));
        assert_eq!(
            retry_delay(&headers, 0, now + Duration::from_secs(20))?,
            Duration::ZERO
        );
        headers.insert(RETRY_AFTER, HeaderValue::from_static("invalid"));
        assert!(retry_delay(&headers, 0, now).is_err());
        Ok(())
    }

    #[test]
    fn musicbrainz_backoff_wait_can_be_cancelled_without_blocking_the_ui() -> Result<()> {
        let (started, ready) = std::sync::mpsc::channel();
        let lookup = Lookup::spawn("Backoff cancellation test", move || async move {
            let mut schedule = RequestSchedule {
                last_request: None,
                cooldown: Some(Instant::now() + Duration::from_secs(60)),
            };
            started.send(())?;
            wait_for_request(&mut schedule).await;
            Ok(())
        });
        ready.recv_timeout(Duration::from_secs(5))?;
        let now = Instant::now();
        drop(lookup);
        assert!(now.elapsed() < Duration::from_secs(1));
        Ok(())
    }

    pub fn sample_result() -> SearchResult {
        SearchResult {
            title: "Related video".into(),
            url: "https://www.youtube.com/watch?v=abcdefghijk".into(),
            uploader: "Artist".into(),
        }
    }

    pub fn sample_artist(id: &str, description: &str) -> ArtistMatch {
        serde_json::from_value(json!({
            "id": id,
            "name": "Artist",
            "disambiguation": description,
        }))
        .unwrap()
    }

    #[test]
    fn ambiguous_artist_choices_fetch_the_selected_id_and_cache_its_songs() {
        let now = Instant::now();
        let mut related = RelatedSongs::default();
        related.update("Artist", "Current", now, 5);
        related.lookup = Some(Lookup::completed(Ok(MusicBrainzResult::Artists(vec![
            sample_artist("first", "First band"),
            sample_artist("second", "Second band"),
        ]))));
        related.update("Artist", "Current", now, 5);
        assert!(!related.loading);
        assert_eq!(related.item_count(), 2);
        related.choose_artist(1);
        assert_eq!(related.selected_artist_id.as_deref(), Some("second"));
        assert!(related.loading);
        assert!(related.artist_matches.is_empty());
        related.lookup = Some(Lookup::completed(Ok(MusicBrainzResult::Recordings(vec![
            "Current".into(),
            "Other".into(),
        ]))));
        related.update("Artist", "Current", now, 5);
        assert_eq!(related.songs, ["Other"]);
        assert!(!related.loading);
        related.update("Another artist", "", now, 5);
        assert!(related.selected_artist_id.is_none());
        related.update("Artist", "Current", now, 5);
        assert_eq!(related.songs, ["Other"]);
        assert!(!related.loading);
    }

    #[test]
    fn musicbrainz_requests_are_spaced_at_least_one_second_apart() -> Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let start = Instant::now();
        let mut schedule = RequestSchedule {
            last_request: Some(start),
            cooldown: Some(start + Duration::from_millis(1200)),
        };
        runtime.block_on(wait_for_request(&mut schedule));
        assert!(schedule.last_request.unwrap().duration_since(start) >= Duration::from_secs(1));
        assert!(
            schedule.last_request.unwrap().duration_since(start) >= Duration::from_millis(1200)
        );
        Ok(())
    }

    #[test]
    #[ignore = "Requires network access to MusicBrainz"]
    fn real_musicbrainz_recordings_lookup() -> Result<()> {
        let lookup = Lookup::spawn("MusicBrainz", move || recordings("Queen".into(), None));
        let result = lookup
            .results
            .recv_timeout(Duration::from_secs(65))?
            .map_err(anyhow::Error::msg)?;
        let titles = match result {
            MusicBrainzResult::Recordings(titles) => titles,
            MusicBrainzResult::Artists(artists) => {
                let artist = artists
                    .into_iter()
                    .find(|artist| artist.disambiguation.as_deref() == Some("UK rock group"))
                    .context("Queen's UK rock group was not in the artist choices")?;
                let lookup = Lookup::spawn("MusicBrainz", move || {
                    recordings("Queen".into(), Some(artist.id))
                });
                match lookup
                    .results
                    .recv_timeout(Duration::from_secs(65))?
                    .map_err(anyhow::Error::msg)?
                {
                    MusicBrainzResult::Recordings(titles) => titles,
                    MusicBrainzResult::Artists(_) => {
                        bail!("Expected recordings for the chosen artist")
                    }
                }
            }
        };
        let limit = crate::config::Settings::default().related_song_limit;
        let songs = other_songs(&titles, "Bohemian Rhapsody", limit);
        assert!(!songs.is_empty());
        assert!(songs.len() <= limit);
        println!("MusicBrainz related recordings: {songs:?}");
        Ok(())
    }

    #[test]
    #[ignore = "Requires yt-dlp and YouTube network access"]
    fn real_youtube_search() -> Result<()> {
        let default_path = crate::library::Library::path()?;
        let (_, settings) = crate::config::Storage::load(
            default_path
                .parent()
                .context("Library path has no parent")?,
        )?;
        let lookup = Lookup::spawn("YouTube search", move || {
            search_youtube("Queen Don't Stop Me Now".into(), settings.executable())
        });
        let results = lookup
            .results
            .recv_timeout(Duration::from_secs(65))?
            .map_err(anyhow::Error::msg)?;
        assert!(!results.is_empty());
        assert!(results.len() <= YOUTUBE_RESULT_LIMIT);
        let mut library = crate::library::Library::default();
        for result in results {
            library.add("Search", &result.title, &result.url)?;
            println!("YouTube search result: {}", result.title);
        }
        Ok(())
    }

    #[test]
    fn musicbrainz_filters_artist_deduplicates_and_limits_other_recordings() -> Result<()> {
        let mut rows = Vec::new();
        for title in [
            "Current", "Song", " song ", "Two", "Three", "Four", "Five", "Six",
        ] {
            rows.push(json!({
                "title": title,
                "artist-credit": [{"name": "Artist", "artist": {"id": "artist-id", "name": "Artist"}}]
            }));
        }
        rows.push(json!({
            "title": "Unrelated",
            "artist-credit": [{"name": "Artist", "artist": {"id": "different-id", "name": "Artist"}}]
        }));
        let response = serde_json::from_value(json!({"recordings": rows}))?;
        let titles = recording_titles(response, "artist-id")?;
        assert_eq!(
            other_songs(&titles, "current", 5),
            ["Song", "Two", "Three", "Four", "Five"]
        );
        assert_eq!(
            artist_query(r#"AC/DC "Live""#),
            r#"artist:"AC\/DC \"Live\"""#
        );
        assert!(serde_json::from_str::<RecordingResponse>("{}").is_err());
        let artists = serde_json::from_value(json!({
            "artists": [
                {"id": "wrong-id", "name": "Artist Band"},
                {"id": "artist-id", "name": "Artist"}
            ]
        }))?;
        assert_eq!(artist_candidates(artists, " artist ")[0].id, "artist-id");
        let ambiguous = serde_json::from_value(json!({
            "artists": [
                {"id": "one", "name": "Artist"},
                {"id": "two", "name": "Artist"}
            ]
        }))?;
        let candidates = artist_candidates(ambiguous, "Artist");
        assert_eq!(candidates.len(), 2);
        assert!(candidates[0].label().contains("one"));
        Ok(())
    }

    #[test]
    fn configured_limit_applies_to_fresh_and_cached_recordings() {
        let now = Instant::now();
        let mut related = RelatedSongs::default();
        related.update("Artist", "Current", now, 10);
        let mut titles = vec!["Current".into()];
        titles.extend((1..=100).map(|index| format!("Song {index}")));
        related.lookup = Some(Lookup::completed(Ok(MusicBrainzResult::Recordings(titles))));
        related.update("Artist", "Current", now, 10);
        assert_eq!(related.songs.len(), 10);
        assert_eq!(related.songs[0], "Song 1");
        assert_eq!(related.songs[9], "Song 10");
        for limit in [1, 100, 5] {
            related.update("Artist", "Current", now, limit);
            assert_eq!(related.songs.len(), limit);
            assert!(!related.loading);
            assert!(related.lookup.is_none());
        }
    }

    #[test]
    fn related_lookup_caches_by_artist_ignores_stale_results_and_supports_retry() {
        let now = Instant::now();
        let mut related = RelatedSongs::default();
        related.update("Artist", "Current", now, 5);
        assert!(related.loading);
        related.lookup = Some(Lookup::completed(Ok(MusicBrainzResult::Recordings(vec![
            "Current".into(),
            "Other".into(),
        ]))));
        related.update("Artist", "Current", now, 5);
        assert_eq!(related.songs, ["Other"]);
        related.update("ARTIST", "Other", now, 5);
        assert_eq!(related.songs, ["Current"]);
        assert!(!related.loading);
        related.lookup = Some(Lookup::completed(Ok(MusicBrainzResult::Recordings(vec![
            "Stale".into(),
        ]))));
        related.update("New artist", "", now, 5);
        assert!(related.songs.is_empty());
        assert!(related.lookup.is_none());
        related.lookup = Some(Lookup::completed(Err("Offline".into())));
        related.update("New artist", "", now, 5);
        assert_eq!(related.error.as_deref(), Some("Offline"));
        related.retry();
        assert!(related.loading);
        assert!(related.error.is_none());
        related.update("", "", now, 5);
        assert!(!related.loading);
        assert!(related.deadline.is_none());
    }

    #[test]
    fn youtube_search_validates_results_and_reports_errors_and_empty_results() -> Result<()> {
        let result = parse_search(br#"{"entries":[null,{"id":"abcdefghijk","title":"Song","channel":"Artist"},{"id":"abcdefghijk","title":"Duplicate"}]}"#)?;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].url, "https://www.youtube.com/watch?v=abcdefghijk");
        assert_eq!(result[0].uploader, "Artist");
        assert!(parse_search(br#"{"entries":[]}"#)?.is_empty());
        assert!(parse_search(br#"{"entries":[{"id":"../invalid","title":"Bad"}]}"#).is_err());
        assert!(parse_search(br#"{"entries":[{"id":"abcdefghijk","title":""}]}"#).is_err());
        assert!(parse_search(b"{}").is_err());
        let entries: Vec<_> = (0..8)
            .map(|index| json!({"id": format!("{index:011}"), "title": format!("Song {index}")}))
            .collect();
        assert_eq!(
            parse_search(&serde_json::to_vec(&json!({"entries": entries}))?)?.len(),
            YOUTUBE_RESULT_LIMIT
        );
        let mut search = YoutubeSearch {
            query: "Artist Song".into(),
            category: "Favorites".into(),
            results: Vec::new(),
            selected: 0,
            error: None,
            lookup: Some(Lookup::completed(Err("Search failed".into()))),
        };
        search.poll();
        assert!(!search.loading());
        assert_eq!(search.error.as_deref(), Some("Search failed"));
        Ok(())
    }
}
