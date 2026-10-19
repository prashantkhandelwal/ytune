use std::{
    collections::HashSet,
    path::PathBuf,
    sync::mpsc::TryRecvError,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

use crate::{
    config::{Settings, Storage},
    library::{Library, Track},
    metadata::{Resolver, TrackMetadata},
    player::{Player, PlayerEvent},
};

const TRACK_SETTLE_DELAY: Duration = Duration::from_millis(400);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Repeat {
    #[default]
    Off,
    Track,
    All,
}

impl Repeat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Track => "Track",
            Self::All => "All",
        }
    }

    fn cycle(self) -> Self {
        match self {
            Self::Off => Self::Track,
            Self::Track => Self::All,
            Self::All => Self::Off,
        }
    }
}

#[derive(Default)]
pub struct Playback {
    pub category: String,
    pub track: Option<Track>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub loading: bool,
    pub active: bool,
    diagnostic: Option<String>,
}

pub struct AddForm {
    pub fields: [String; 3],
    pub focus: usize,
    pub error: Option<String>,
}

pub struct PathForm {
    pub path: String,
    pub error: Option<String>,
}

pub struct DeleteConfirmation {
    pub category: String,
    pub track: Track,
    pub error: Option<String>,
}

pub struct SettingsForm {
    pub fields: [String; 5],
    pub focus: usize,
    pub error: Option<String>,
    original_ytdlp: String,
    original_mpv: String,
}

impl SettingsForm {
    fn values(&self, current: &Settings) -> Result<(Settings, PathBuf)> {
        let path = self.fields[0].trim();
        let ytdlp_path = if self.fields[0] == self.original_ytdlp {
            current.ytdlp_path.clone()
        } else if path.is_empty() {
            None
        } else {
            Some(
                which::which(path)
                    .context("yt-dlp executable was not found at the specified path")?,
            )
        };
        let path = self.fields[1].trim();
        let mpv_path = if self.fields[1] == self.original_mpv {
            current.mpv_path.clone()
        } else if path.is_empty() {
            None
        } else {
            Some(which::which(path).context("mpv executable was not found at the specified path")?)
        };
        let settings = Settings {
            ytdlp_path,
            mpv_path,
            seek_seconds: self.fields[2]
                .trim()
                .parse()
                .context("Seek interval must be a positive whole number of seconds")?,
            volume_step: self.fields[3]
                .trim()
                .parse()
                .context("Volume step must be a whole number between 1 and 100")?,
        };
        settings.validate()?;
        let directory = self.fields[4].trim();
        if directory.is_empty() {
            anyhow::bail!("Enter a data folder");
        }
        Ok((settings, PathBuf::from(directory)))
    }
}

impl PathForm {
    fn edit(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Backspace => {
                self.path.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.path.clear()
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.path.push(c)
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LibraryRow {
    Category(usize),
    Track(usize),
}

pub struct App {
    pub settings: Settings,
    pub storage: Storage,
    pub settings_form: Option<SettingsForm>,
    pub player_shortcuts_open: bool,
    pub delete_confirmation: Option<DeleteConfirmation>,
    pub library: Library,
    pub path: PathBuf,
    pub selected: usize,
    pub selected_header: Option<usize>,
    pub collapsed: HashSet<String>,
    pub playback: Playback,
    pub repeat: Repeat,
    pub volume: f64,
    pub muted: bool,
    pub ready: bool,
    pub status: String,
    pub error: Option<String>,
    pub form: Option<AddForm>,
    pub missing_ytdlp: Option<String>,
    pub export_form: Option<PathForm>,
    pub import_form: Option<PathForm>,
    pub quit: bool,
    player: Player,
    resolver: Option<Resolver>,
    pending_playback: Option<Instant>,
}

impl App {
    pub fn new() -> Result<Self> {
        let default_path = Library::path()?;
        let (storage, settings) = Storage::load(
            default_path
                .parent()
                .context("Library path has no parent")?,
        )?;
        let path = storage.library_path();
        let library = Library::load(&path)?;
        let player = Player::start(settings.executable(), settings.mpv_executable());
        Ok(Self {
            settings,
            storage,
            settings_form: None,
            player_shortcuts_open: false,
            delete_confirmation: None,
            library,
            path,
            selected: 0,
            selected_header: None,
            collapsed: HashSet::new(),
            playback: Playback::default(),
            repeat: Repeat::Off,
            volume: 70.0,
            muted: false,
            ready: false,
            status: "Starting playback engine...".into(),
            error: None,
            form: None,
            missing_ytdlp: None,
            export_form: None,
            import_form: None,
            quit: false,
            player,
            resolver: None,
            pending_playback: None,
        })
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.missing_ytdlp.is_some() {
            match key.code {
                KeyCode::Esc => self.missing_ytdlp = None,
                KeyCode::Char('t') => {
                    self.missing_ytdlp = None;
                    self.open_settings();
                }
                KeyCode::Char('q') if key.modifiers.is_empty() => self.quit = true,
                _ => {}
            }
            return;
        }
        if self.delete_confirmation.is_some() {
            if key.modifiers.is_empty() {
                match key.code {
                    KeyCode::Char('y') => self.confirm_remove(),
                    KeyCode::Char('n') | KeyCode::Esc => self.delete_confirmation = None,
                    KeyCode::Char('q') => self.quit = true,
                    _ => {}
                }
            }
            return;
        }
        if self.settings_form.is_some() {
            self.handle_settings(key);
            return;
        }
        if self.form.is_some() {
            self.handle_form(key);
            return;
        }
        if self.export_form.is_some() {
            self.handle_export(key);
            return;
        }
        if self.import_form.is_some() {
            self.handle_import(key);
            return;
        }
        if self.player_shortcuts_open {
            match key.code {
                KeyCode::Esc | KeyCode::Char('k') if key.modifiers.is_empty() => {
                    self.player_shortcuts_open = false;
                    return;
                }
                KeyCode::Char(' ' | 'n' | 'b' | 'r' | 'm' | 's' | 'q' | 'j' | '+' | '=' | '-')
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Left
                | KeyCode::Right => {}
                _ => return,
            }
        }
        match key.code {
            KeyCode::Char('q') if key.modifiers.is_empty() => self.quit = true,
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection(1);
            }
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('k') if key.modifiers.is_empty() => self.player_shortcuts_open = true,
            KeyCode::Home => {
                if let Some(row) = self.visible_rows().first() {
                    self.select_row(*row);
                }
            }
            KeyCode::End => {
                if let Some(row) = self.visible_rows().last() {
                    self.select_row(*row);
                }
            }
            KeyCode::Tab => self.toggle_category(),
            KeyCode::Char(' ') => {
                if self.selected_header.is_some() {
                    self.toggle_category();
                    return;
                }
                let selected_is_current =
                    self.library
                        .get(self.selected)
                        .is_some_and(|(category, track)| {
                            self.playback.category == category
                                && self
                                    .playback
                                    .track
                                    .as_ref()
                                    .is_some_and(|t| t.url == track.url)
                        });
                if self.playback.active && selected_is_current && self.pending_playback.is_none() {
                    self.send(json!(["cycle", "pause"]));
                } else {
                    self.play(self.selected, false);
                }
            }
            KeyCode::Char('n') => self.advance(false, false),
            KeyCode::Char('b') => self.advance(true, false),
            KeyCode::Char('r') => self.repeat = self.repeat.cycle(),
            KeyCode::Left if self.playback.active && self.pending_playback.is_none() => {
                self.send(json!([
                    "seek",
                    -i64::from(self.settings.seek_seconds),
                    "relative"
                ]));
            }
            KeyCode::Right if self.playback.active && self.pending_playback.is_none() => {
                self.send(json!(["seek", self.settings.seek_seconds, "relative"]));
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.change_volume(f64::from(self.settings.volume_step))
            }
            KeyCode::Char('-') => self.change_volume(-f64::from(self.settings.volume_step)),
            KeyCode::Char('m') => {
                self.send(json!(["cycle", "mute"]));
            }
            KeyCode::Char('t') => self.open_settings(),
            KeyCode::Char('s') => self.stop(),
            KeyCode::Char('a') => {
                let category = self
                    .library
                    .get(self.selected)
                    .map(|(name, _)| name)
                    .unwrap_or("Favorites");
                self.form = Some(AddForm {
                    fields: [category.into(), String::new(), String::new()],
                    focus: 2,
                    error: None,
                });
            }
            KeyCode::Char('e') => {
                self.export_form = Some(PathForm {
                    path: self
                        .storage
                        .directory
                        .join("ytune-playlists.json")
                        .to_string_lossy()
                        .into_owned(),
                    error: None,
                });
            }
            KeyCode::Char('i') => {
                self.import_form = Some(PathForm {
                    path: String::new(),
                    error: None,
                });
            }
            KeyCode::Delete => self.request_remove(),
            _ => {}
        }
    }

    fn open_settings(&mut self) {
        let executable = self.settings.displayed_executable();
        let mpv = self.settings.displayed_mpv_executable();
        self.settings_form = Some(SettingsForm {
            fields: [
                executable.clone(),
                mpv.clone(),
                self.settings.seek_seconds.to_string(),
                self.settings.volume_step.to_string(),
                self.storage.directory.to_string_lossy().into_owned(),
            ],
            focus: 0,
            error: None,
            original_ytdlp: executable,
            original_mpv: mpv,
        });
    }

    fn handle_settings(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.settings_form = None;
            return;
        }
        if key.code == KeyCode::Enter {
            self.save_settings();
            return;
        }
        let Some(form) = &mut self.settings_form else {
            return;
        };
        match key.code {
            KeyCode::Tab | KeyCode::Down => form.focus = (form.focus + 1) % form.fields.len(),
            KeyCode::BackTab | KeyCode::Up => {
                form.focus = (form.focus + form.fields.len() - 1) % form.fields.len()
            }
            KeyCode::Backspace => {
                form.fields[form.focus].pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                form.fields[form.focus].clear()
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                form.fields[form.focus].push(c)
            }
            _ => {}
        }
    }

    fn save_settings(&mut self) {
        let Some(form) = &self.settings_form else {
            return;
        };
        let result = form
            .values(&self.settings)
            .and_then(|(settings, directory)| {
                let storage = self.storage.save(&directory, &settings, &self.library)?;
                Ok((settings, storage))
            });
        match result {
            Ok((settings, storage)) => {
                let restart = settings.ytdlp_path != self.settings.ytdlp_path
                    || settings.mpv_path != self.settings.mpv_path
                    || !self.ready;
                self.settings = settings;
                self.path = storage.library_path();
                self.storage = storage;
                self.settings_form = None;
                self.status = "Settings saved".into();
                if restart {
                    self.pending_playback = None;
                    self.player.shutdown();
                    self.muted = false;
                    self.ready = false;
                    self.error = None;
                    self.missing_ytdlp = None;
                    self.playback = Playback::default();
                    self.player =
                        Player::start(self.settings.executable(), self.settings.mpv_executable());
                    self.status = "Restarting playback engine with saved settings...".into();
                }
            }
            Err(error) => {
                if let Some(form) = &mut self.settings_form {
                    form.error = Some(format!("{error:#}"));
                }
            }
        }
    }

    fn handle_form(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.resolver = None;
            self.form = None;
            return;
        }
        if self.resolving() {
            return;
        }
        if key.code == KeyCode::Enter {
            self.save_form();
            return;
        }
        let Some(form) = &mut self.form else {
            return;
        };
        match key.code {
            KeyCode::Tab | KeyCode::Down => form.focus = (form.focus + 1) % 3,
            KeyCode::BackTab | KeyCode::Up => form.focus = (form.focus + 2) % 3,
            KeyCode::Backspace => {
                form.fields[form.focus].pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                form.fields[form.focus].clear();
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                form.fields[form.focus].push(c);
            }
            _ => {}
        }
    }

    pub fn handle_paste(&mut self, text: &str) {
        if self.resolving() || self.missing_ytdlp.is_some() {
            return;
        }
        if let Some(form) = &mut self.settings_form {
            form.fields[form.focus].extend(text.chars().filter(|c| !c.is_control()));
        } else if let Some(form) = &mut self.form {
            form.fields[form.focus].extend(text.chars().filter(|c| !c.is_control()));
        } else if let Some(form) = self.export_form.as_mut().or(self.import_form.as_mut()) {
            form.path.extend(text.chars().filter(|c| !c.is_control()));
        }
    }

    pub fn resolving(&self) -> bool {
        self.resolver.is_some()
    }

    fn save_form(&mut self) {
        let Some(form) = &self.form else {
            return;
        };
        let mut validation = self.library.clone();
        if let Err(error) = validation.add(&form.fields[0], &form.fields[1], &form.fields[2]) {
            if let Some(form) = &mut self.form {
                form.error = Some(format!("{error:#}"));
            }
            return;
        }
        let url = form.fields[2].trim().to_string();
        if let Some(form) = &mut self.form {
            form.error = None;
        }
        self.resolver = Some(Resolver::start(url, self.settings.executable()));
    }

    fn finish_add(&mut self, result: Result<TrackMetadata, String>) {
        let Some(form) = &self.form else {
            return;
        };
        let metadata = match result {
            Ok(metadata) => metadata,
            Err(error) => {
                if let Some(form) = &mut self.form {
                    form.error = Some(error);
                }
                return;
            }
        };
        let mut updated = self.library.clone();
        let result = updated
            .add_resolved(&form.fields[0], &form.fields[1], &form.fields[2], metadata)
            .and_then(|index| {
                updated.save(&self.path)?;
                Ok(index)
            });
        match result {
            Ok(index) => {
                self.library = updated;
                self.selected = index;
                self.selected_header = None;
                if let Some((category, _)) = self.library.get(index) {
                    self.collapsed.remove(category);
                }
                self.form = None;
                self.status = "Track saved to library".into();
            }
            Err(error) => {
                if let Some(form) = &mut self.form {
                    form.error = Some(format!("{error:#}"));
                }
            }
        }
    }

    fn handle_export(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.export_form = None;
            return;
        }
        if key.code == KeyCode::Enter {
            let Some(form) = &self.export_form else {
                return;
            };
            let path = PathBuf::from(form.path.trim());
            let result = if path.as_os_str().is_empty() {
                Err(anyhow::anyhow!("Enter an export destination path"))
            } else {
                self.library.export(&path)
            };
            match result {
                Ok(()) => {
                    self.status = format!("Library exported to {}", path.display());
                    self.export_form = None;
                }
                Err(error) => {
                    if let Some(form) = &mut self.export_form {
                        form.error = Some(format!("{error:#}"));
                    }
                }
            }
            return;
        }
        let Some(form) = &mut self.export_form else {
            return;
        };
        form.edit(key);
    }

    fn handle_import(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.import_form = None;
            return;
        }
        if key.code == KeyCode::Enter {
            let Some(form) = &self.import_form else {
                return;
            };
            let path = PathBuf::from(form.path.trim());
            let result = if path.as_os_str().is_empty() {
                Err(anyhow::anyhow!("Enter the path to a playlist JSON file"))
            } else {
                self.library
                    .merge_file(&path)
                    .and_then(|(updated, summary)| {
                        if summary.added > 0 {
                            updated.save(&self.path)?;
                        }
                        Ok((updated, summary))
                    })
            };
            match result {
                Ok((updated, summary)) => {
                    let selected = self
                        .library
                        .get(self.selected)
                        .map(|(category, track)| (category.to_string(), track.url.clone()));
                    self.library = updated;
                    if let Some(category) = self.selected_header {
                        self.select_row(LibraryRow::Category(category));
                    } else if let Some((category, url)) = selected
                        && let Some(index) = self.library.index_of(&category, &url)
                    {
                        self.selected = index;
                    }
                    self.status = format!(
                        "Imported {} tracks; skipped {} duplicates",
                        summary.added, summary.skipped
                    );
                    self.import_form = None;
                }
                Err(error) => {
                    if let Some(form) = &mut self.import_form {
                        form.error = Some(format!("{error:#}"));
                    }
                }
            }
            return;
        }
        if let Some(form) = &mut self.import_form {
            form.edit(key);
        }
    }

    pub fn visible_rows(&self) -> Vec<LibraryRow> {
        let mut rows = Vec::new();
        let mut index = 0;
        for (category, playlist) in self.library.playlists.iter().enumerate() {
            rows.push(LibraryRow::Category(category));
            if !self.collapsed.contains(&playlist.name) {
                rows.extend((index..index + playlist.tracks.len()).map(LibraryRow::Track));
            }
            index += playlist.tracks.len();
        }
        rows
    }

    pub fn selected_row(&self) -> LibraryRow {
        self.selected_header
            .map(LibraryRow::Category)
            .unwrap_or(LibraryRow::Track(self.selected))
    }

    fn select_row(&mut self, row: LibraryRow) {
        match row {
            LibraryRow::Category(category) => {
                self.selected_header = Some(category);
                self.selected = self
                    .library
                    .playlists
                    .iter()
                    .take(category)
                    .map(|p| p.tracks.len())
                    .sum();
            }
            LibraryRow::Track(index) => {
                self.selected = index;
                self.selected_header = None;
            }
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let rows = self.visible_rows();
        if let Some(position) = rows.iter().position(|row| *row == self.selected_row()) {
            let position = position
                .saturating_add_signed(delta)
                .min(rows.len().saturating_sub(1));
            self.select_row(rows[position]);
        }
    }

    fn toggle_category(&mut self) {
        let category = self.selected_header.or_else(|| {
            let (name, _) = self.library.get(self.selected)?;
            self.library.playlists.iter().position(|p| p.name == name)
        });
        let Some(category) = category else { return };
        let name = self.library.playlists[category].name.clone();
        if !self.collapsed.remove(&name) {
            self.collapsed.insert(name);
        }
        self.select_row(LibraryRow::Category(category));
    }

    fn request_remove(&mut self) {
        if self.selected_header.is_some() {
            self.status = "Expand the category and select a track to remove".into();
            return;
        }
        let Some((category, track)) = self.library.get(self.selected) else {
            return;
        };
        self.delete_confirmation = Some(DeleteConfirmation {
            category: category.to_string(),
            track: track.clone(),
            error: None,
        });
    }

    fn confirm_remove(&mut self) {
        let Some(confirmation) = &self.delete_confirmation else {
            return;
        };
        let Some(index) = self
            .library
            .index_of(&confirmation.category, &confirmation.track.url)
        else {
            self.delete_confirmation.as_mut().unwrap().error = Some(
                "This track is no longer in the library. Cancel and select a track again.".into(),
            );
            return;
        };
        let category = confirmation.category.as_str();
        let track = &confirmation.track;
        let is_current = self.playback.category == category
            && self
                .playback
                .track
                .as_ref()
                .is_some_and(|t| t.url == track.url);
        let mut updated = self.library.clone();
        updated.remove(index);
        if let Err(error) = updated.save(&self.path) {
            self.delete_confirmation.as_mut().unwrap().error =
                Some(format!("Cannot remove track: {error:#}"));
            return;
        }
        self.delete_confirmation = None;
        self.library = updated;
        self.selected = self.selected.min(self.library.len().saturating_sub(1));
        self.collapsed
            .retain(|name| self.library.playlists.iter().any(|p| p.name == *name));
        if let Some((name, _)) = self.library.get(self.selected) {
            self.selected_header = self
                .library
                .playlists
                .iter()
                .position(|p| p.name == name)
                .filter(|&i| self.collapsed.contains(&self.library.playlists[i].name));
        }
        if is_current {
            self.pending_playback = None;
            self.send(json!(["stop"]));
            self.playback = Playback::default();
        }
        self.status = "Track removed".into();
    }

    fn send(&mut self, command: Value) -> bool {
        if !self.ready {
            if self.error.is_none() {
                self.error = Some("Playback engine is not ready yet".into());
            }
            return false;
        }
        if let Err(error) = self.player.command(command) {
            self.error = Some(format!("{error:#}"));
            self.ready = false;
            return false;
        }
        true
    }

    fn stop(&mut self) {
        self.pending_playback = None;
        if self.send(json!(["stop"])) {
            self.playback.diagnostic = None;
            self.playback.active = false;
            self.playback.loading = false;
            self.playback.paused = false;
            self.playback.position = 0.0;
            self.error = None;
            self.status = "Playback stopped".into();
        }
    }

    fn play(&mut self, index: usize, immediate: bool) {
        let Some((category, track)) = self.library.get(index) else {
            self.status = "Press a to add a YouTube video".into();
            return;
        };
        let category = category.to_string();
        let track = track.clone();
        if !self.ready {
            if self.error.is_none() {
                self.error = Some("Playback engine is not ready yet".into());
            }
            return;
        }
        let now = Instant::now();
        self.pending_playback = Some(if immediate {
            now
        } else {
            now + TRACK_SETTLE_DELAY
        });
        self.selected = index;
        self.selected_header = self
            .library
            .playlists
            .iter()
            .position(|p| p.name == category)
            .filter(|_| self.collapsed.contains(&category));
        let stored = track.metadata.as_ref();
        self.playback = Playback {
            title: track.title.clone(),
            artist: stored
                .map(|m| m.artist_label().to_string())
                .unwrap_or_default(),
            album: stored.and_then(|m| m.album.clone()).unwrap_or_default(),
            duration: stored.and_then(|m| m.duration).unwrap_or_default(),
            category,
            track: Some(track),
            loading: true,
            active: true,
            ..Playback::default()
        };
        self.error = None;
        self.status = "Waiting for track selection to settle...".into();
        if immediate {
            self.start_pending_playback(now);
        }
    }

    fn start_pending_playback(&mut self, now: Instant) {
        if self.quit || self.pending_playback.is_none_or(|deadline| now < deadline) {
            return;
        }
        self.pending_playback = None;
        let Some(track) = &self.playback.track else {
            return;
        };
        if self.send(json!(["loadfile", track.url, "replace"]))
            && self.send(json!(["set_property", "pause", false]))
        {
            self.status = "Resolving YouTube audio...".into();
        } else {
            self.playback.active = false;
            self.playback.loading = false;
        }
    }

    fn advance(&mut self, backwards: bool, automatic: bool) {
        let current = self
            .playback
            .track
            .as_ref()
            .and_then(|track| self.library.index_of(&self.playback.category, &track.url));
        let next = next_index(
            self.library.len(),
            current,
            self.selected,
            backwards,
            automatic,
            self.repeat,
        );
        if let Some(index) = next {
            self.play(index, automatic);
        } else {
            self.status = "Queue finished. Press Space to play again".into();
        }
    }

    fn change_volume(&mut self, delta: f64) {
        let volume = (self.volume + delta).clamp(0.0, 100.0);
        if self.send(json!(["set_property", "volume", volume])) {
            self.volume = volume;
        }
    }

    pub fn poll_player(&mut self) {
        if let Some(resolver) = &self.resolver {
            let result = match resolver.results.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Err("Metadata resolver stopped unexpectedly".into()))
                }
            };
            if let Some(result) = result {
                self.resolver = None;
                self.finish_add(result);
            }
        }
        while let Ok(event) = self.player.events.try_recv() {
            match event {
                PlayerEvent::Ready => {
                    self.ready = true;
                    self.missing_ytdlp = None;
                    self.error = None;
                    self.status = "Ready. Press a to add audio, Space to play".into();
                }
                PlayerEvent::YtdlpMissing(error) => {
                    self.pending_playback = None;
                    self.missing_ytdlp = Some(error.clone());
                    self.error = Some(error);
                    self.ready = false;
                    self.playback.active = false;
                    self.playback.loading = false;
                }
                PlayerEvent::Error(error) => {
                    self.pending_playback = None;
                    self.error = Some(error);
                    self.ready = false;
                    self.playback.active = false;
                    self.playback.loading = false;
                }
                PlayerEvent::Message(message) => self.handle_message(&message),
            }
        }
        self.start_pending_playback(Instant::now());
    }

    fn handle_message(&mut self, message: &Value) {
        // The old track can still emit events while a replacement is waiting.
        if self.pending_playback.is_some()
            && match message["event"].as_str() {
                Some("property-change") => {
                    !matches!(message["name"].as_str(), Some("volume" | "mute"))
                }
                Some("file-loaded" | "end-file" | "log-message") => true,
                _ => false,
            }
        {
            return;
        }
        if let Some(error) = message["error"].as_str().filter(|&e| e != "success") {
            self.error = Some(format!("mpv: {error}"));
            self.playback.loading = false;
        }
        match message["event"].as_str() {
            Some("property-change") => self.property_changed(message),
            Some("start-file") => self.playback.diagnostic = None,
            Some("file-loaded") if self.playback.active => {
                self.playback.diagnostic = None;
                self.playback.loading = false;
                self.status = "Streaming audio".into();
            }
            Some("end-file") => match message["reason"].as_str() {
                Some("eof") if self.playback.active => {
                    self.playback.diagnostic = None;
                    self.playback.active = false;
                    self.playback.loading = false;
                    self.playback.position = self.playback.duration;
                    self.advance(false, true);
                }
                Some("error") => {
                    self.playback.active = false;
                    self.playback.loading = false;
                    let reason = message["file_error"]
                        .as_str()
                        .unwrap_or("unknown mpv error");
                    let detail = self
                        .playback
                        .diagnostic
                        .take()
                        .map(|diagnostic| format!("{reason}: {diagnostic}"))
                        .unwrap_or_else(|| reason.to_string());
                    self.error = Some(format!(
                        "Playback failed: {detail}. Check the URL, network, and yt-dlp version; press Space to retry"
                    ));
                }
                Some("stop" | "quit") => self.playback.diagnostic = None,
                _ => {}
            },
            Some("log-message") => {
                // mpv can recover from probe errors or cancel an old extractor.
                // Only end-file with reason=error confirms a playback failure.
                if self.playback.active
                    && let Some(text) = message["text"]
                        .as_str()
                        .filter(|text| !text.trim().is_empty())
                {
                    self.playback.diagnostic = Some(text.trim().to_string());
                }
            }
            _ => {}
        }
    }

    fn property_changed(&mut self, message: &Value) {
        let data = &message["data"];
        match message["name"].as_str() {
            Some("pause") => {
                if let Some(paused) = data.as_bool() {
                    self.playback.paused = paused;
                }
            }
            Some("volume") => {
                if let Some(volume) = data.as_f64() {
                    self.volume = volume;
                }
            }
            Some("mute") => {
                if let Some(muted) = data.as_bool() {
                    self.muted = muted;
                }
            }
            Some("media-title") => {
                if let Some(title) = data.as_str() {
                    self.playback.title = title.to_string();
                }
            }
            Some("duration") => {
                if let Some(duration) = data.as_f64() {
                    self.playback.duration = duration;
                }
            }
            Some("time-pos") if self.playback.active => {
                if let Some(position) = data.as_f64() {
                    self.playback.position = position;
                }
            }
            Some("metadata") => {
                self.playback.artist = metadata(data, &["artist", "ARTIST", "uploader"]);
                self.playback.album = metadata(data, &["album", "ALBUM"]);
                if let Some(stored) = self
                    .playback
                    .track
                    .as_ref()
                    .and_then(|track| track.metadata.as_ref())
                {
                    if self.playback.artist.is_empty() {
                        self.playback.artist = stored.artist_label().to_string();
                    }
                    if self.playback.album.is_empty() {
                        self.playback.album = stored.album.clone().unwrap_or_default();
                    }
                }
            }
            _ => {}
        }
    }
}

fn metadata(data: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| data[*key].as_str())
        .unwrap_or_default()
        .to_string()
}

fn next_index(
    len: usize,
    current: Option<usize>,
    selected: usize,
    backwards: bool,
    automatic: bool,
    repeat: Repeat,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let Some(current) = current else {
        return Some(selected.min(len - 1));
    };
    if automatic && repeat == Repeat::Track {
        return Some(current);
    }
    if backwards {
        return Some(if current == 0 { len - 1 } else { current - 1 });
    }
    if current + 1 < len {
        Some(current + 1)
    } else if !automatic || repeat == Repeat::All {
        Some(0)
    } else {
        None
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn fixture() -> (
        App,
        tempfile::TempDir,
        std::sync::mpsc::Receiver<Value>,
        std::sync::mpsc::Sender<PlayerEvent>,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let (player, commands, events) = Player::test_channels();
        let app = App {
            settings: Settings::default(),
            storage: Storage::at(
                directory.path().to_path_buf(),
                directory.path().join("location.json"),
            ),
            settings_form: None,
            player_shortcuts_open: false,
            delete_confirmation: None,
            library: Library::default(),
            path: directory.path().join("library.json"),
            selected: 0,
            selected_header: None,
            collapsed: HashSet::new(),
            playback: Playback::default(),
            repeat: Repeat::Off,
            volume: 70.0,
            muted: false,
            ready: true,
            status: "Ready".into(),
            error: None,
            form: None,
            missing_ytdlp: None,
            export_form: None,
            import_form: None,
            quit: false,
            player,
            resolver: None,
            pending_playback: None,
        };
        (app, directory, commands, events)
    }

    fn key(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
        // Control tests use settled playback; debounce tests send keys directly.
        app.start_pending_playback(Instant::now() + TRACK_SETTLE_DELAY);
    }

    #[test]
    fn rapid_track_changes_only_load_the_final_selection_after_settling() -> Result<()> {
        let (mut app, _directory, commands, _events) = fixture();
        for (title, url) in [
            ("One", "https://youtu.be/one"),
            ("Two", "https://youtu.be/two"),
            ("Three", "https://youtu.be/three"),
        ] {
            app.library.add("Test", title, url)?;
        }
        for code in [
            KeyCode::Char(' '),
            KeyCode::Char('n'),
            KeyCode::Char('n'),
            KeyCode::Char('b'),
            KeyCode::Char('n'),
        ] {
            let requested_at = Instant::now();
            app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
            let deadline = app.pending_playback.unwrap();
            assert!(deadline >= requested_at + TRACK_SETTLE_DELAY);
            app.poll_player();
            app.start_pending_playback(deadline - Duration::from_millis(1));
            assert!(commands.try_recv().is_err());
        }
        assert_eq!(app.selected, 2);
        assert_eq!(app.playback.title, "Three");
        let deadline = app.pending_playback.unwrap();
        for message in [
            json!({"event": "file-loaded"}),
            json!({"event": "end-file", "reason": "eof"}),
            json!({"event": "end-file", "reason": "error", "file_error": "TLS error"}),
            json!({"event": "log-message", "text": "TLS error"}),
            json!({"event": "property-change", "name": "media-title", "data": "Old title"}),
        ] {
            app.handle_message(&message);
        }
        assert_eq!(app.playback.title, "Three");
        assert!(app.playback.loading);
        assert!(app.error.is_none());
        assert_eq!(app.pending_playback, Some(deadline));
        app.start_pending_playback(deadline);
        assert_eq!(
            commands.try_recv()?,
            json!(["loadfile", "https://youtu.be/three", "replace"])
        );
        assert_eq!(
            commands.try_recv()?,
            json!(["set_property", "pause", false])
        );
        app.start_pending_playback(deadline + TRACK_SETTLE_DELAY);
        assert!(commands.try_recv().is_err());
        Ok(())
    }

    #[test]
    fn pending_playback_keeps_global_controls_but_does_not_pause_or_seek_old_audio() -> Result<()> {
        let (mut app, _directory, commands, events) = fixture();
        app.library.add("Test", "One", "https://youtu.be/one")?;
        for code in [
            KeyCode::Char(' '),
            KeyCode::Char(' '),
            KeyCode::Left,
            KeyCode::Right,
        ] {
            app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
        }
        assert!(commands.try_recv().is_err());
        app.handle_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE));
        assert_eq!(
            commands.try_recv()?,
            json!(["set_property", "volume", 75.0])
        );
        for (name, data) in [("volume", json!(75.0)), ("mute", json!(true))] {
            events.send(PlayerEvent::Message(json!({
                "event": "property-change", "name": name, "data": data
            })))?;
        }
        app.poll_player();
        assert_eq!(app.volume, 75.0);
        assert!(app.muted);
        app.pending_playback = Some(Instant::now());
        app.poll_player();
        assert_eq!(
            commands.try_recv()?,
            json!(["loadfile", "https://youtu.be/one", "replace"])
        );
        assert_eq!(
            commands.try_recv()?,
            json!(["set_property", "pause", false])
        );
        assert!(app.pending_playback.is_none());
        Ok(())
    }

    #[test]
    fn pending_track_changes_are_cancelled_by_stop_removal_and_engine_failure() -> Result<()> {
        for action in ["stop", "remove", "error", "missing-ytdlp", "quit"] {
            let (mut app, _directory, commands, events) = fixture();
            app.library.add("Test", "One", "https://youtu.be/one")?;
            app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
            let deadline = app.pending_playback.unwrap();
            match action {
                "stop" => {
                    key(&mut app, KeyCode::Char('s'));
                    assert_eq!(commands.try_recv()?, json!(["stop"]));
                }
                "remove" => {
                    app.request_remove();
                    app.confirm_remove();
                    assert_eq!(commands.try_recv()?, json!(["stop"]));
                }
                "error" => {
                    events.send(PlayerEvent::Error("Engine disconnected".into()))?;
                    app.poll_player();
                    assert_eq!(app.error.as_deref(), Some("Engine disconnected"));
                }
                "missing-ytdlp" => {
                    events.send(PlayerEvent::YtdlpMissing("yt-dlp missing".into()))?;
                    app.poll_player();
                    assert!(app.missing_ytdlp.is_some());
                }
                "quit" => key(&mut app, KeyCode::Char('q')),
                _ => unreachable!(),
            }
            app.start_pending_playback(deadline);
            assert!(
                commands.try_recv().is_err(),
                "{action} must cancel the load"
            );
        }
        Ok(())
    }

    #[test]
    #[ignore = "Requires mpv, yt-dlp and network access; set YTUNE_SMOKE_URL"]
    fn real_track_switch_keeps_playback_error_free() -> Result<()> {
        let url = std::env::var("YTUNE_SMOKE_URL").context("Set YTUNE_SMOKE_URL")?;
        let (mut app, _directory, _commands, _events) = fixture();
        app.player = Player::start(crate::metadata::executable(), app.settings.mpv_executable());
        app.ready = false;
        app.library.add("First", "First selection", &url)?;
        app.library.add("Second", "Final selection", &url)?;
        loop {
            match app.player.events.recv_timeout(Duration::from_secs(15))? {
                PlayerEvent::Ready => {
                    app.ready = true;
                    break;
                }
                PlayerEvent::Error(error) | PlayerEvent::YtdlpMissing(error) => {
                    anyhow::bail!("{error}");
                }
                PlayerEvent::Message(message) => app.handle_message(&message),
            }
        }
        app.play(0, true);
        loop {
            match app.player.events.recv_timeout(Duration::from_secs(15))? {
                PlayerEvent::Message(message) => {
                    let started = message["event"] == "start-file";
                    app.handle_message(&message);
                    if started {
                        break;
                    }
                }
                PlayerEvent::Error(error) | PlayerEvent::YtdlpMissing(error) => {
                    anyhow::bail!("{error}");
                }
                PlayerEvent::Ready => {}
            }
        }
        std::thread::sleep(Duration::from_millis(1500));
        for shortcut in ['n', 'b', 'n'] {
            app.handle_key(KeyEvent::new(KeyCode::Char(shortcut), KeyModifiers::NONE));
            std::thread::sleep(TRACK_SETTLE_DELAY);
            app.poll_player();
            assert!(
                app.error.is_none(),
                "Unexpected playback error: {:?}",
                app.error
            );
        }
        assert_eq!(app.selected, 1);
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            match app.player.events.recv_timeout(Duration::from_millis(500)) {
                Ok(PlayerEvent::Message(message)) => {
                    if message["event"] == "log-message" {
                        println!("mpv diagnostic: {message}");
                    }
                    app.handle_message(&message);
                    assert!(
                        app.error.is_none(),
                        "Unexpected playback error: {:?}",
                        app.error
                    );
                    if app.playback.position >= 0.5 {
                        println!("Final selection is streaming without a cancellation error");
                        return Ok(());
                    }
                }
                Ok(PlayerEvent::Error(error) | PlayerEvent::YtdlpMissing(error)) => {
                    anyhow::bail!("{error}");
                }
                Ok(PlayerEvent::Ready) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => return Err(error.into()),
            }
        }
        anyhow::bail!("Timed out waiting for the final selection to play")
    }

    #[test]
    fn controls_and_playback_events() -> Result<()> {
        let (mut app, _directory, commands, events) = fixture();
        app.library.add("Test", "One", "https://youtu.be/one")?;
        app.library.add("Test", "Two", "https://youtu.be/two")?;
        key(&mut app, KeyCode::Enter);
        assert!(commands.try_recv().is_err());
        assert!(!app.playback.active);
        key(&mut app, KeyCode::Char(' '));
        assert_eq!(
            commands.recv()?,
            json!(["loadfile", "https://youtu.be/one", "replace"])
        );
        assert_eq!(commands.recv()?, json!(["set_property", "pause", false]));
        assert!(app.playback.loading);
        events.send(PlayerEvent::Message(json!({"event": "file-loaded"})))?;
        events.send(PlayerEvent::Message(json!({
            "event": "property-change", "name": "media-title", "data": "Resolved title"
        })))?;
        events.send(PlayerEvent::Message(json!({
            "event": "property-change", "name": "duration", "data": 120.0
        })))?;
        app.poll_player();
        assert!(!app.playback.loading);
        assert_eq!(app.playback.title, "Resolved title");
        assert_eq!(app.playback.duration, 120.0);
        key(&mut app, KeyCode::Char(' '));
        assert_eq!(commands.recv()?, json!(["cycle", "pause"]));
        key(&mut app, KeyCode::Right);
        assert_eq!(commands.recv()?, json!(["seek", 10, "relative"]));
        app.handle_message(&json!({"event": "end-file", "reason": "eof"}));
        assert_eq!(app.selected, 1);
        assert_eq!(
            commands.recv()?,
            json!(["loadfile", "https://youtu.be/two", "replace"])
        );
        commands.recv()?;
        app.handle_message(&json!({"event": "end-file", "reason": "eof"}));
        assert!(!app.playback.active);
        assert!(commands.try_recv().is_err());
        Ok(())
    }

    #[test]
    fn saving_form_paste_removal_and_failures() -> Result<()> {
        let (mut app, directory, commands, _events) = fixture();
        key(&mut app, KeyCode::Char('a'));
        app.handle_paste("https://youtu.be/one\r\n");
        app.finish_add(Ok(crate::metadata::tests::sample()));
        assert_eq!(Library::load(&app.path)?.len(), 1);
        assert!(app.form.is_none());
        key(&mut app, KeyCode::Char(' '));
        commands.recv()?;
        commands.recv()?;
        key(&mut app, KeyCode::Delete);
        assert!(app.delete_confirmation.is_some());
        assert!(commands.try_recv().is_err());
        key(&mut app, KeyCode::Char('y'));
        assert_eq!(commands.recv()?, json!(["stop"]));
        assert_eq!(app.library.len(), 0);
        assert!(app.playback.track.is_none());
        assert_eq!(Library::load(&app.path)?.len(), 0);

        // An invalid save destination must not change the working library.
        app.path = directory.path().to_path_buf();
        key(&mut app, KeyCode::Char('a'));
        app.handle_paste("https://youtu.be/two");
        app.finish_add(Ok(crate::metadata::tests::sample()));
        assert_eq!(app.library.len(), 0);
        assert!(app.form.as_ref().unwrap().error.is_some());
        key(&mut app, KeyCode::Esc);
        assert!(app.form.is_none());
        Ok(())
    }

    #[test]
    fn deletion_requires_confirmation_and_cancellation_preserves_library_and_playback() -> Result<()>
    {
        let (mut app, _directory, commands, _events) = fixture();
        app.library.add("Test", "One", "https://youtu.be/one")?;
        app.library.save(&app.path)?;
        key(&mut app, KeyCode::Char(' '));
        commands.try_recv()?;
        commands.try_recv()?;
        for cancel in [KeyCode::Esc, KeyCode::Char('n')] {
            key(&mut app, KeyCode::Delete);
            for blocked in [
                KeyCode::Delete,
                KeyCode::Enter,
                KeyCode::Down,
                KeyCode::Char('s'),
            ] {
                key(&mut app, blocked);
                assert!(app.delete_confirmation.is_some());
                assert_eq!(app.library.len(), 1);
                assert_eq!(Library::load(&app.path)?.len(), 1);
                assert!(app.playback.active);
                assert!(commands.try_recv().is_err());
            }
            key(&mut app, cancel);
            assert!(app.delete_confirmation.is_none());
            assert_eq!(app.library.len(), 1);
            assert!(app.playback.active);
            assert!(!app.quit);
        }
        app.selected_header = Some(0);
        key(&mut app, KeyCode::Delete);
        assert!(app.delete_confirmation.is_none());
        Ok(())
    }

    #[test]
    fn deletion_retries_save_failures_and_targets_original_song_after_queue_advances() -> Result<()>
    {
        let (mut app, directory, commands, _events) = fixture();
        app.library.add("Test", "One", "https://youtu.be/one")?;
        app.library.add("Test", "Two", "https://youtu.be/two")?;
        app.library.save(&app.path)?;
        let original_path = app.path.clone();
        key(&mut app, KeyCode::Char(' '));
        commands.try_recv()?;
        commands.try_recv()?;
        key(&mut app, KeyCode::Delete);
        app.path = directory.path().to_path_buf();
        key(&mut app, KeyCode::Char('y'));
        assert!(app.delete_confirmation.as_ref().unwrap().error.is_some());
        assert_eq!(app.library.len(), 2);
        assert_eq!(Library::load(&original_path)?.len(), 2);
        assert!(app.playback.active);
        assert!(commands.try_recv().is_err());
        app.path = original_path;
        app.handle_message(&json!({"event": "end-file", "reason": "eof"}));
        assert_eq!(app.selected, 1);
        commands.try_recv()?;
        commands.try_recv()?;
        key(&mut app, KeyCode::Char('y'));
        assert!(app.delete_confirmation.is_none());
        assert_eq!(app.library.len(), 1);
        assert_eq!(app.library.get(0).unwrap().1.title, "Two");
        assert_eq!(Library::load(&app.path)?.len(), 1);
        assert!(app.playback.active);
        assert_eq!(app.playback.track.as_ref().unwrap().title, "Two");
        assert!(commands.try_recv().is_err());
        app.ready = false;
        key(&mut app, KeyCode::Delete);
        key(&mut app, KeyCode::Char('n'));
        assert_eq!(app.library.len(), 1);
        Ok(())
    }

    #[test]
    fn errors_remain_visible_and_do_not_skip_tracks() -> Result<()> {
        let (mut app, _directory, commands, events) = fixture();
        app.library.add("Test", "One", "https://youtu.be/one")?;
        key(&mut app, KeyCode::Char(' '));
        commands.recv()?;
        commands.recv()?;
        app.handle_message(&json!({
            "event": "end-file", "reason": "error", "file_error": "Network failure"
        }));
        assert!(!app.playback.active);
        assert!(app.error.as_ref().unwrap().contains("Network failure"));
        assert!(commands.try_recv().is_err());
        events.send(PlayerEvent::Error("Engine disconnected".into()))?;
        app.poll_player();
        key(&mut app, KeyCode::Char(' '));
        assert!(!app.ready);
        assert_eq!(app.error.as_deref(), Some("Engine disconnected"));
        assert!(commands.try_recv().is_err());
        Ok(())
    }

    #[test]
    fn recoverable_mpv_logs_do_not_become_sticky_playback_errors() -> Result<()> {
        let (mut app, _directory, commands, _events) = fixture();
        app.library.add("Test", "One", "https://youtu.be/one")?;
        key(&mut app, KeyCode::Char(' '));
        commands.try_recv()?;
        commands.try_recv()?;
        for text in ["tls: Unknown error\n", "subprocess failed: killed\n"] {
            app.handle_message(&json!({
                "event": "log-message", "level": "error", "text": text
            }));
            assert!(app.error.is_none());
            assert!(app.playback.active);
            assert!(app.playback.loading);
            assert_eq!(app.playback.diagnostic.as_deref(), Some(text.trim()));
        }
        app.handle_message(&json!({"event": "end-file", "reason": "stop"}));
        assert!(app.playback.diagnostic.is_none());
        app.handle_message(&json!({
            "event": "log-message", "text": "subprocess failed: killed\n"
        }));
        app.handle_message(&json!({"event": "start-file"}));
        assert!(app.playback.diagnostic.is_none());
        app.handle_message(&json!({
            "event": "log-message", "text": "tls: Unknown error\n"
        }));
        app.handle_message(&json!({"event": "file-loaded"}));
        assert!(app.error.is_none());
        assert!(app.playback.diagnostic.is_none());
        assert!(!app.playback.loading);
        assert_eq!(app.status, "Streaming audio");
        app.handle_message(&json!({
            "event": "property-change", "name": "time-pos", "data": 1.0
        }));
        assert_eq!(app.playback.position, 1.0);
        assert!(commands.try_recv().is_err());
        Ok(())
    }

    #[test]
    fn next_and_previous_during_resolution_tolerate_late_extractor_cancellation() -> Result<()> {
        for cancellation_after_start in [false, true] {
            let (mut app, _directory, commands, _events) = fixture();
            app.library.add("Test", "One", "https://youtu.be/one")?;
            app.library.add("Test", "Two", "https://youtu.be/two")?;
            key(&mut app, KeyCode::Char(' '));
            commands.try_recv()?;
            commands.try_recv()?;
            for (shortcut, url, title) in [
                ('n', "https://youtu.be/two", "Two"),
                ('b', "https://youtu.be/one", "One"),
            ] {
                assert!(app.playback.loading);
                key(&mut app, KeyCode::Char(shortcut));
                assert_eq!(commands.try_recv()?, json!(["loadfile", url, "replace"]));
                assert_eq!(
                    commands.try_recv()?,
                    json!(["set_property", "pause", false])
                );
                app.handle_message(&json!({"event": "end-file", "reason": "stop"}));
                if cancellation_after_start {
                    app.handle_message(&json!({"event": "start-file"}));
                }
                app.handle_message(&json!({
                    "event": "log-message", "prefix": "cplayer", "level": "error",
                    "text": "subprocess failed: killed\n"
                }));
                if !cancellation_after_start {
                    app.handle_message(&json!({"event": "start-file"}));
                }
                assert!(app.error.is_none());
                assert!(app.playback.active);
                assert!(app.playback.loading);
                assert_eq!(app.playback.title, title);
                assert!(commands.try_recv().is_err());
            }
            app.handle_message(&json!({"event": "file-loaded"}));
            app.handle_message(&json!({
                "event": "property-change", "name": "time-pos", "data": 1.0
            }));
            assert!(app.error.is_none());
            assert!(app.playback.active);
            assert!(!app.playback.loading);
            assert!(app.playback.diagnostic.is_none());
            assert_eq!(app.playback.position, 1.0);
        }
        Ok(())
    }

    #[test]
    fn actual_playback_failures_include_diagnostics_and_are_not_overwritten_by_logs() -> Result<()>
    {
        for text in ["TLS handshake failed", "subprocess failed: killed"] {
            let (mut app, _directory, commands, _events) = fixture();
            app.library.add("Test", "One", "https://youtu.be/one")?;
            key(&mut app, KeyCode::Char(' '));
            commands.try_recv()?;
            commands.try_recv()?;
            app.handle_message(&json!({"event": "log-message", "text": text}));
            app.handle_message(&json!({
                "event": "end-file", "reason": "error", "file_error": "unrecognized file format"
            }));
            let error = app.error.clone().unwrap();
            assert!(error.contains("Playback failed"));
            assert!(error.contains("unrecognized file format"));
            assert!(error.contains(text));
            assert!(!app.playback.active);
            assert!(!app.playback.loading);
            assert!(app.playback.diagnostic.is_none());
            app.handle_message(&json!({
                "event": "log-message", "text": "subprocess failed: killed"
            }));
            assert_eq!(app.error.as_deref(), Some(error.as_str()));
            assert!(commands.try_recv().is_err());
        }
        Ok(())
    }

    #[test]
    fn mpv_command_errors_are_still_reported_immediately() -> Result<()> {
        let (mut app, _directory, commands, _events) = fixture();
        app.library.add("Test", "One", "https://youtu.be/one")?;
        key(&mut app, KeyCode::Char(' '));
        commands.try_recv()?;
        commands.try_recv()?;
        app.handle_message(&json!({"error": "command failed"}));
        assert_eq!(app.error.as_deref(), Some("mpv: command failed"));
        app.handle_message(&json!({"event": "file-loaded"}));
        assert_eq!(app.error.as_deref(), Some("mpv: command failed"));
        Ok(())
    }

    #[test]
    fn space_plays_new_selection_instead_of_pausing_previous_track() -> Result<()> {
        let (mut app, _directory, commands, _events) = fixture();
        app.library.add("Test", "One", "https://youtu.be/one")?;
        app.library.add("Test", "Two", "https://youtu.be/two")?;
        key(&mut app, KeyCode::Char(' '));
        commands.recv()?;
        commands.recv()?;
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        assert!(commands.try_recv().is_err());
        key(&mut app, KeyCode::Char(' '));
        assert_eq!(
            commands.recv()?,
            json!(["loadfile", "https://youtu.be/two", "replace"])
        );
        assert_eq!(commands.recv()?, json!(["set_property", "pause", false]));
        key(&mut app, KeyCode::Char(' '));
        assert_eq!(commands.recv()?, json!(["cycle", "pause"]));
        Ok(())
    }

    #[test]
    fn categories_collapse_and_navigation_skips_hidden_tracks_without_changing_queue() -> Result<()>
    {
        let (mut app, _directory, commands, _events) = fixture();
        app.library.add("First", "One", "https://youtu.be/one")?;
        app.library.add("First", "Two", "https://youtu.be/two")?;
        app.library
            .add("Second", "Three", "https://youtu.be/three")?;
        key(&mut app, KeyCode::Char(' '));
        commands.recv()?;
        commands.recv()?;
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.selected_row(), LibraryRow::Category(0));
        assert_eq!(
            app.visible_rows(),
            vec![
                LibraryRow::Category(0),
                LibraryRow::Category(1),
                LibraryRow::Track(2)
            ]
        );
        key(&mut app, KeyCode::Down);
        assert_eq!(app.selected_row(), LibraryRow::Category(1));
        key(&mut app, KeyCode::Up);
        key(&mut app, KeyCode::Char('n'));
        assert_eq!(app.selected, 1);
        assert_eq!(app.selected_header, Some(0));
        assert!(app.collapsed.contains("First"));
        assert_eq!(
            commands.recv()?,
            json!(["loadfile", "https://youtu.be/two", "replace"])
        );
        commands.recv()?;
        key(&mut app, KeyCode::Tab);
        key(&mut app, KeyCode::Down);
        assert_eq!(app.selected_row(), LibraryRow::Track(0));
        assert_eq!(app.library.len(), 3);
        Ok(())
    }

    #[test]
    fn export_dialog_writes_entire_library_and_refuses_overwrite() -> Result<()> {
        let (mut app, directory, _commands, _events) = fixture();
        app.library.add_resolved(
            "First",
            "",
            "https://youtu.be/one",
            crate::metadata::tests::sample(),
        )?;
        app.library.add("Second", "Two", "https://youtu.be/two")?;
        let destination = directory.path().join("export.json");
        for attempt in 0..2 {
            key(&mut app, KeyCode::Char('e'));
            app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
            app.handle_paste(&destination.to_string_lossy());
            key(&mut app, KeyCode::Enter);
            if attempt == 0 {
                assert!(app.export_form.is_none());
            } else {
                assert!(app.export_form.as_ref().unwrap().error.is_some());
            }
        }
        let exported = Library::load(&destination)?;
        assert_eq!(exported.playlists.len(), 2);
        assert_eq!(
            exported.get(0).unwrap().1.metadata.as_ref().unwrap().title,
            "Resolved title"
        );
        key(&mut app, KeyCode::Esc);
        assert!(app.export_form.is_none());
        Ok(())
    }

    #[test]
    fn importing_preserves_selection_playback_and_collapsed_categories() -> Result<()> {
        let (mut app, directory, commands, _events) = fixture();
        app.library.add("First", "One", "https://youtu.be/one")?;
        app.library.add("Second", "Two", "https://youtu.be/two")?;
        app.selected = 1;
        key(&mut app, KeyCode::Char(' '));
        commands.recv()?;
        commands.recv()?;
        app.collapsed.insert("First".into());
        let mut source = Library::default();
        source.add_resolved(
            "First",
            "",
            "https://youtu.be/new",
            crate::metadata::tests::sample(),
        )?;
        source.add("Second", "Duplicate", "https://youtu.be/two")?;
        let path = directory.path().join("import.json");
        source.export(&path)?;
        key(&mut app, KeyCode::Char('i'));
        app.handle_paste(&path.to_string_lossy());
        key(&mut app, KeyCode::Enter);
        assert!(app.import_form.is_none());
        assert_eq!(app.library.len(), 3);
        assert_eq!(Library::load(&app.path)?.len(), 3);
        assert_eq!(app.selected, 2);
        assert_eq!(
            app.library.get(app.selected).unwrap().1.url,
            "https://youtu.be/two"
        );
        assert_eq!(
            app.playback.track.as_ref().unwrap().url,
            "https://youtu.be/two"
        );
        assert!(app.playback.active);
        assert!(app.collapsed.contains("First"));
        assert!(commands.try_recv().is_err());
        assert_eq!(app.status, "Imported 1 tracks; skipped 1 duplicates");
        Ok(())
    }

    #[test]
    fn import_failures_and_cancellation_leave_library_unchanged_and_work_offline() -> Result<()> {
        let (mut app, directory, _commands, _events) = fixture();
        app.ready = false;
        app.library
            .add("Existing", "Keep", "https://youtu.be/keep")?;
        app.library.save(&app.path)?;
        let original_path = app.path.clone();
        let before = std::fs::read(&original_path)?;
        key(&mut app, KeyCode::Char('i'));
        key(&mut app, KeyCode::Enter);
        assert!(app.import_form.as_ref().unwrap().error.is_some());
        app.handle_paste(&directory.path().join("missing.json").to_string_lossy());
        key(&mut app, KeyCode::Enter);
        assert!(app.import_form.as_ref().unwrap().error.is_some());
        key(&mut app, KeyCode::Esc);
        assert!(app.import_form.is_none());
        assert!(!app.quit);
        let mut source = Library::default();
        source.add("New", "New", "https://youtu.be/new")?;
        let source_path = directory.path().join("import.json");
        source.export(&source_path)?;
        key(&mut app, KeyCode::Char('i'));
        app.handle_paste(&source_path.to_string_lossy());
        app.path = directory.path().to_path_buf();
        key(&mut app, KeyCode::Enter);
        assert!(app.import_form.as_ref().unwrap().error.is_some());
        assert_eq!(app.library.len(), 1);
        assert_eq!(std::fs::read(&original_path)?, before);
        app.path = original_path;
        key(&mut app, KeyCode::Enter);
        assert!(app.import_form.is_none());
        assert_eq!(app.library.len(), 2);
        assert!(!app.ready);
        Ok(())
    }

    #[test]
    fn resolved_metadata_is_saved_and_failures_keep_form_open() -> Result<()> {
        let (mut app, _directory, _commands, _events) = fixture();
        key(&mut app, KeyCode::Char('a'));
        key(&mut app, KeyCode::Enter);
        assert!(app.form.as_ref().unwrap().error.is_some());
        assert!(!app.resolving());
        app.handle_paste("https://youtu.be/one");
        app.resolver = Some(Resolver::completed(Err("Unavailable video".into())));
        app.poll_player();
        assert_eq!(app.library.len(), 0);
        assert_eq!(
            app.form.as_ref().unwrap().error.as_deref(),
            Some("Unavailable video")
        );
        app.resolver = Some(Resolver::completed(Ok(crate::metadata::tests::sample())));
        app.poll_player();
        let saved = Library::load(&app.path)?;
        let track = saved.get(0).unwrap().1;
        assert_eq!(track.title, "Resolved title");
        assert_eq!(track.metadata.as_ref().unwrap().artist_label(), "Artist");
        assert_eq!(track.metadata.as_ref().unwrap().duration, Some(120.0));
        assert!(app.form.is_none());
        key(&mut app, KeyCode::Char('a'));
        app.handle_paste("https://youtu.be/two");
        app.resolver = Some(Resolver::completed(Ok(crate::metadata::tests::sample())));
        key(&mut app, KeyCode::Esc);
        app.poll_player();
        assert_eq!(app.library.len(), 1);
        assert!(!app.resolving());
        Ok(())
    }

    #[test]
    #[ignore = "Requires yt-dlp and network access; set YTUNE_SMOKE_URL"]
    fn real_add_resolves_and_persists_metadata() -> Result<()> {
        use std::time::{Duration, Instant};

        let (mut app, directory, _commands, _events) = fixture();
        let url = std::env::var("YTUNE_SMOKE_URL")?;
        key(&mut app, KeyCode::Char('a'));
        app.handle_paste(&url);
        key(&mut app, KeyCode::Enter);
        assert!(app.resolving());
        let deadline = Instant::now() + Duration::from_secs(65);
        while app.resolving() && Instant::now() < deadline {
            app.poll_player();
            std::thread::sleep(Duration::from_millis(25));
        }
        if let Some(form) = &app.form {
            anyhow::bail!("Could not save video: {:?}", form.error);
        }
        let library = Library::load(&app.path)?;
        let track = library.get(0).unwrap().1;
        let metadata = track.metadata.as_ref().unwrap();
        assert_eq!(track.title, metadata.title);
        assert_ne!(track.title, url);
        assert!(metadata.duration.is_some_and(|duration| duration > 0.0));
        assert!(metadata.uploader.is_some());
        let destination = directory.path().join("resolved-export.json");
        library.export(&destination)?;
        assert_eq!(Library::load(&destination)?.get(0).unwrap().1, track);
        println!("Saved and exported resolved title: {}", track.title);

        key(&mut app, KeyCode::Char('a'));
        app.handle_paste("https://youtu.be/cancelled");
        key(&mut app, KeyCode::Enter);
        std::thread::sleep(Duration::from_millis(100));
        let cancellation = Instant::now();
        key(&mut app, KeyCode::Esc);
        assert!(cancellation.elapsed() < Duration::from_secs(3));
        assert!(!app.resolving());
        assert_eq!(Library::load(&app.path)?.len(), 1);
        println!("Verified metadata lookup cancellation without saving a track");
        Ok(())
    }

    #[test]
    fn missing_ytdlp_opens_dismissible_error_and_keeps_engine_not_ready() -> Result<()> {
        let (mut app, _directory, _commands, events) = fixture();
        events.send(PlayerEvent::YtdlpMissing(
            "Install yt-dlp or set YTUNE_YTDLP".into(),
        ))?;
        app.poll_player();
        assert!(!app.ready);
        assert!(app.missing_ytdlp.is_some());
        assert_eq!(app.error, app.missing_ytdlp);
        key(&mut app, KeyCode::Char('a'));
        assert!(app.form.is_none());
        key(&mut app, KeyCode::Esc);
        assert!(app.missing_ytdlp.is_none());
        assert!(!app.quit);
        assert!(!app.ready);
        key(&mut app, KeyCode::Char('e'));
        assert!(app.export_form.is_some());
        key(&mut app, KeyCode::Esc);
        events.send(PlayerEvent::Ready)?;
        app.poll_player();
        assert!(app.ready);
        assert!(app.error.is_none());
        assert!(app.missing_ytdlp.is_none());
        Ok(())
    }

    #[test]
    fn settings_screen_saves_steps_and_playback_uses_them_immediately() -> Result<()> {
        let (mut app, directory, commands, _events) = fixture();
        key(&mut app, KeyCode::Char('t'));
        assert!(app.settings_form.is_some());
        key(&mut app, KeyCode::Tab);
        key(&mut app, KeyCode::Tab);
        app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        app.handle_paste("25");
        key(&mut app, KeyCode::Tab);
        app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        app.handle_paste("7");
        key(&mut app, KeyCode::Enter);
        assert!(app.settings_form.is_none());
        assert_eq!(app.settings.seek_seconds, 25);
        assert_eq!(app.settings.volume_step, 7);
        assert_eq!(Storage::load(directory.path())?.1, app.settings);
        app.library.add("Test", "One", "https://youtu.be/one")?;
        key(&mut app, KeyCode::Char(' '));
        commands.recv()?;
        commands.recv()?;
        key(&mut app, KeyCode::Left);
        assert_eq!(commands.recv()?, json!(["seek", -25, "relative"]));
        key(&mut app, KeyCode::Right);
        assert_eq!(commands.recv()?, json!(["seek", 25, "relative"]));
        key(&mut app, KeyCode::Char('+'));
        assert_eq!(commands.recv()?, json!(["set_property", "volume", 77.0]));
        key(&mut app, KeyCode::Char('-'));
        assert_eq!(commands.recv()?, json!(["set_property", "volume", 70.0]));
        app.volume = 98.0;
        key(&mut app, KeyCode::Char('+'));
        assert_eq!(commands.recv()?, json!(["set_property", "volume", 100.0]));
        Ok(())
    }

    #[test]
    fn mpv_settings_preserve_discovery_resolve_paths_and_clear_overrides() -> Result<()> {
        let (mut app, _directory, _commands, _events) = fixture();
        key(&mut app, KeyCode::Char('t'));
        let form = app.settings_form.as_mut().unwrap();
        assert_eq!(
            form.fields,
            [
                app.settings.displayed_executable(),
                app.settings.displayed_mpv_executable(),
                app.settings.seek_seconds.to_string(),
                app.settings.volume_step.to_string(),
                app.storage.directory.to_string_lossy().into_owned(),
            ]
        );
        assert_eq!(form.values(&app.settings)?.0.mpv_path, None);
        let executable = std::env::current_exe()?;
        form.fields[1] = executable.to_string_lossy().into_owned();
        assert_eq!(
            form.values(&app.settings)?.0.mpv_path,
            Some(which::which(&executable)?)
        );
        app.settings.mpv_path = Some(executable.clone());
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Char('t'));
        let form = app.settings_form.as_mut().unwrap();
        assert_eq!(form.values(&app.settings)?.0.mpv_path, Some(executable));
        form.fields[1].clear();
        assert_eq!(form.values(&app.settings)?.0.mpv_path, None);
        key(&mut app, KeyCode::Up);
        assert_eq!(app.settings_form.as_ref().unwrap().focus, 4);
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.settings_form.as_ref().unwrap().focus, 0);
        Ok(())
    }

    #[test]
    fn changing_mpv_settings_persists_path_and_restarts_playback() -> Result<()> {
        let (mut app, directory, commands, _events) = fixture();
        app.settings.ytdlp_path = Some(directory.path().join("missing-yt-dlp.exe"));
        app.playback.active = true;
        app.muted = true;
        let executable = std::env::current_exe()?;
        key(&mut app, KeyCode::Char('t'));
        app.settings_form.as_mut().unwrap().fields[1] = executable.to_string_lossy().into_owned();
        key(&mut app, KeyCode::Enter);
        assert!(app.settings_form.is_none());
        assert_eq!(commands.recv()?, json!(["quit"]));
        assert!(!app.ready);
        assert!(!app.playback.active);
        assert!(!app.muted);
        assert_eq!(app.settings.mpv_path, Some(which::which(executable)?));
        assert_eq!(Storage::load(directory.path())?.1, app.settings);
        app.player.shutdown();
        Ok(())
    }

    #[test]
    fn settings_relocation_updates_library_and_default_export_paths() -> Result<()> {
        let (mut app, directory, _commands, _events) = fixture();
        app.library.add("Old", "One", "https://youtu.be/one")?;
        app.library.save(&app.path)?;
        let original_path = app.path.clone();
        let target = directory.path().join("new folder");
        key(&mut app, KeyCode::Char('t'));
        app.settings_form.as_mut().unwrap().fields[4] = target.to_string_lossy().into_owned();
        key(&mut app, KeyCode::Enter);
        assert!(app.settings_form.is_none());
        assert_eq!(app.path, target.join("library.json"));
        assert_eq!(Storage::load(directory.path())?.0.directory, target);
        key(&mut app, KeyCode::Char('a'));
        app.handle_paste("https://youtu.be/two");
        app.finish_add(Ok(crate::metadata::tests::sample()));
        assert_eq!(Library::load(&app.path)?.len(), 2);
        assert_eq!(Library::load(&original_path)?.len(), 1);
        key(&mut app, KeyCode::Char('e'));
        assert_eq!(
            PathBuf::from(&app.export_form.as_ref().unwrap().path),
            target.join("ytune-playlists.json")
        );
        Ok(())
    }

    #[test]
    fn invalid_settings_and_cancellation_do_not_change_saved_values() -> Result<()> {
        let (mut app, directory, _commands, _events) = fixture();
        app.storage
            .save(directory.path(), &app.settings, &app.library)?;
        let before = std::fs::read(app.storage.settings_path())?;
        for (field, value) in [
            (2, "0"),
            (2, "abc"),
            (3, "101"),
            (4, ""),
            (0, "missing-yt-dlp-executable.exe"),
            (1, "missing-mpv-executable.exe"),
        ] {
            key(&mut app, KeyCode::Char('t'));
            app.settings_form.as_mut().unwrap().fields[field] = value.to_string();
            key(&mut app, KeyCode::Enter);
            assert!(app.settings_form.as_ref().unwrap().error.is_some());
            assert_eq!(app.settings, Settings::default());
            assert_eq!(std::fs::read(app.storage.settings_path())?, before);
            key(&mut app, KeyCode::Esc);
            assert!(app.settings_form.is_none());
        }
        Ok(())
    }

    #[test]
    fn settings_can_be_opened_directly_from_the_missing_dependency_dialog() -> Result<()> {
        let (mut app, _directory, _commands, events) = fixture();
        events.send(PlayerEvent::YtdlpMissing("Install yt-dlp".into()))?;
        app.poll_player();
        key(&mut app, KeyCode::Char('t'));
        assert!(app.missing_ytdlp.is_none());
        assert!(app.settings_form.is_some());
        assert!(!app.ready);
        key(&mut app, KeyCode::Esc);
        assert!(!app.quit);
        assert!(app.settings_form.is_none());
        Ok(())
    }

    #[test]
    #[ignore = "Requires mpv and network; set YTUNE_TEST_YTDLP and YTUNE_SMOKE_URL"]
    fn real_settings_path_reconfigures_player_and_metadata_lookup() -> Result<()> {
        use std::time::{Duration, Instant};

        let (mut app, directory, _commands, events) = fixture();
        events.send(PlayerEvent::YtdlpMissing("Not installed on PATH".into()))?;
        app.poll_player();
        key(&mut app, KeyCode::Char('t'));
        let executable = std::env::var("YTUNE_TEST_YTDLP")?;
        let data_dir = directory.path().join("configured data");
        let form = app.settings_form.as_mut().unwrap();
        form.fields[0] = executable;
        form.fields[2] = "15".into();
        form.fields[3] = "8".into();
        form.fields[4] = data_dir.to_string_lossy().into_owned();
        key(&mut app, KeyCode::Enter);
        assert!(
            app.settings_form.is_none(),
            "Settings error: {:?}",
            app.settings_form
                .as_ref()
                .and_then(|form| form.error.as_ref())
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        while !app.ready && Instant::now() < deadline {
            app.poll_player();
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(app.ready, "Player failed to restart: {:?}", app.error);
        assert_eq!(app.settings.seek_seconds, 15);
        assert_eq!(app.settings.volume_step, 8);
        let (storage, saved) = Storage::load(directory.path())?;
        assert_eq!(storage.directory, data_dir);
        assert_eq!(saved, app.settings);
        key(&mut app, KeyCode::Char('a'));
        app.handle_paste(&std::env::var("YTUNE_SMOKE_URL")?);
        key(&mut app, KeyCode::Enter);
        let deadline = Instant::now() + Duration::from_secs(65);
        while app.resolving() && Instant::now() < deadline {
            app.poll_player();
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            app.form.is_none(),
            "Metadata error: {:?}",
            app.form.as_ref().and_then(|form| form.error.as_ref())
        );
        assert_eq!(Library::load(&storage.library_path())?.len(), 1);
        assert!(app.library.get(0).unwrap().1.metadata.is_some());
        key(&mut app, KeyCode::Char(' '));
        let deadline = Instant::now() + Duration::from_secs(60);
        while app.playback.position < 0.5 && Instant::now() < deadline {
            app.poll_player();
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            app.playback.position >= 0.5,
            "Playback did not advance: {:?}",
            app.error
        );
        println!(
            "Verified settings-based yt-dlp recovery, relocation, metadata persistence, and streaming"
        );
        Ok(())
    }

    #[test]
    fn m_toggles_mpv_mute_and_observed_state_does_not_change_volume() -> Result<()> {
        let (mut app, _directory, commands, events) = fixture();
        for muted in [true, false] {
            key(&mut app, KeyCode::Char('m'));
            assert_eq!(commands.recv()?, json!(["cycle", "mute"]));
            events.send(PlayerEvent::Message(json!({
                "event": "property-change", "name": "mute", "data": muted
            })))?;
            app.poll_player();
            assert_eq!(app.muted, muted);
            assert_eq!(app.volume, 70.0);
        }
        app.ready = false;
        key(&mut app, KeyCode::Char('m'));
        assert!(commands.try_recv().is_err());
        assert!(!app.muted);
        assert!(app.error.is_some());
        Ok(())
    }

    #[test]
    fn s_stops_audio_without_advancing_and_t_opens_settings() -> Result<()> {
        let (mut app, _directory, commands, _events) = fixture();
        app.library.add("Test", "One", "https://youtu.be/one")?;
        app.library.add("Test", "Two", "https://youtu.be/two")?;
        key(&mut app, KeyCode::Char(' '));
        commands.recv()?;
        commands.recv()?;
        app.repeat = Repeat::All;
        app.playback.position = 12.0;
        app.playback.paused = true;
        app.muted = true;
        key(&mut app, KeyCode::Char('s'));
        assert_eq!(commands.recv()?, json!(["stop"]));
        assert!(!app.playback.active);
        assert!(!app.playback.loading);
        assert!(!app.playback.paused);
        assert_eq!(app.playback.position, 0.0);
        assert_eq!(
            app.playback.track.as_ref().unwrap().url,
            "https://youtu.be/one"
        );
        assert!(app.settings_form.is_none());
        assert!(app.ready);
        assert!(app.muted);
        assert_eq!(app.volume, 70.0);
        for message in [
            json!({"event": "end-file", "reason": "stop"}),
            json!({"event": "end-file", "reason": "eof"}),
            json!({"event": "file-loaded"}),
            json!({"event": "property-change", "name": "time-pos", "data": 12.0}),
        ] {
            app.handle_message(&message);
        }
        assert!(commands.try_recv().is_err());
        assert_eq!(app.status, "Playback stopped");
        assert_eq!(app.playback.position, 0.0);
        key(&mut app, KeyCode::Char(' '));
        assert_eq!(
            commands.recv()?,
            json!(["loadfile", "https://youtu.be/one", "replace"])
        );
        assert_eq!(commands.recv()?, json!(["set_property", "pause", false]));
        key(&mut app, KeyCode::Char('t'));
        assert!(app.settings_form.is_some());
        assert!(commands.try_recv().is_err());
        Ok(())
    }

    #[test]
    fn player_shortcuts_dialog_keeps_controls_live_and_preserves_text_entry() -> Result<()> {
        let (mut app, _directory, commands, _events) = fixture();
        app.library.add("Test", "One", "https://youtu.be/one")?;
        app.library.add("Test", "Two", "https://youtu.be/two")?;
        app.selected = 1;
        for close in [KeyCode::Esc, KeyCode::Char('k')] {
            key(&mut app, KeyCode::Char('k'));
            assert!(app.player_shortcuts_open);
            assert_eq!(app.selected, 1);
            key(&mut app, KeyCode::Up);
            assert_eq!(app.selected, 0);
            key(&mut app, KeyCode::Down);
            assert_eq!(app.selected, 1);
            key(&mut app, KeyCode::Char('+'));
            assert_eq!(
                commands.try_recv()?,
                json!(["set_property", "volume", 75.0])
            );
            key(&mut app, KeyCode::Char('-'));
            assert_eq!(
                commands.try_recv()?,
                json!(["set_property", "volume", 70.0])
            );
            key(&mut app, KeyCode::Char('a'));
            assert_eq!(app.selected, 1);
            assert!(app.form.is_none());
            key(&mut app, KeyCode::Char(' '));
            assert_eq!(
                commands.try_recv()?,
                json!(["loadfile", "https://youtu.be/two", "replace"])
            );
            assert_eq!(
                commands.try_recv()?,
                json!(["set_property", "pause", false])
            );
            key(&mut app, KeyCode::Left);
            assert_eq!(commands.try_recv()?, json!(["seek", -10, "relative"]));
            key(&mut app, KeyCode::Right);
            assert_eq!(commands.try_recv()?, json!(["seek", 10, "relative"]));
            key(&mut app, KeyCode::Char('m'));
            assert_eq!(commands.try_recv()?, json!(["cycle", "mute"]));
            for (shortcut, url) in [('n', "https://youtu.be/one"), ('b', "https://youtu.be/two")] {
                key(&mut app, KeyCode::Char(shortcut));
                assert_eq!(commands.try_recv()?, json!(["loadfile", url, "replace"]));
                assert_eq!(
                    commands.try_recv()?,
                    json!(["set_property", "pause", false])
                );
            }
            let expected_repeat = app.repeat.cycle();
            key(&mut app, KeyCode::Char('r'));
            assert_eq!(app.repeat, expected_repeat);
            key(&mut app, KeyCode::Char('s'));
            assert_eq!(commands.try_recv()?, json!(["stop"]));
            assert!(app.player_shortcuts_open);
            key(&mut app, close);
            assert!(!app.player_shortcuts_open);
            assert!(!app.quit);
        }
        key(&mut app, KeyCode::Up);
        assert_eq!(app.selected, 0);
        key(&mut app, KeyCode::Char('a'));
        key(&mut app, KeyCode::Char('k'));
        assert!(!app.player_shortcuts_open);
        assert_eq!(app.form.as_ref().unwrap().fields[2], "k");
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Char('k'));
        key(&mut app, KeyCode::Char('q'));
        assert!(app.quit);
        Ok(())
    }

    #[test]
    fn only_plain_q_quits_and_escape_only_dismisses_dialogs() {
        let (mut app, _directory, _commands, _events) = fixture();
        for shortcut in ['a', 'e', 'i', 't'] {
            key(&mut app, KeyCode::Char(shortcut));
            app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
            assert!(!app.quit);
            key(&mut app, KeyCode::Esc);
            assert!(!app.quit);
            assert!(app.form.is_none());
            assert!(app.export_form.is_none());
            assert!(app.import_form.is_none());
            assert!(app.settings_form.is_none());
            key(&mut app, KeyCode::Esc);
            assert!(!app.quit);
        }
        for code in ['c', 'q'] {
            app.handle_key(KeyEvent::new(KeyCode::Char(code), KeyModifiers::CONTROL));
            assert!(!app.quit);
        }
        key(&mut app, KeyCode::Char('q'));
        assert!(app.quit);
    }

    #[test]
    fn queue_navigation_and_repeat_modes() {
        assert_eq!(next_index(0, None, 0, false, false, Repeat::Off), None);
        assert_eq!(next_index(3, None, 1, false, false, Repeat::Off), Some(1));
        assert_eq!(next_index(3, Some(1), 0, false, true, Repeat::Off), Some(2));
        assert_eq!(next_index(3, Some(2), 0, false, true, Repeat::Off), None);
        assert_eq!(next_index(3, Some(2), 0, false, true, Repeat::All), Some(0));
        assert_eq!(
            next_index(3, Some(1), 0, false, true, Repeat::Track),
            Some(1)
        );
        assert_eq!(
            next_index(3, Some(1), 0, false, false, Repeat::Track),
            Some(2)
        );
        assert_eq!(next_index(3, Some(0), 0, true, false, Repeat::Off), Some(2));
        assert_eq!(next_index(1, Some(0), 0, false, true, Repeat::Off), None);
        assert_eq!(next_index(1, Some(0), 0, false, true, Repeat::All), Some(0));
        assert_eq!(Repeat::Off.cycle().cycle().cycle(), Repeat::Off);
    }

    #[test]
    fn metadata_accepts_mpv_tag_casing() {
        assert_eq!(
            metadata(&json!({"ARTIST": "Artist"}), &["artist", "ARTIST"]),
            "Artist"
        );
        assert_eq!(metadata(&Value::Null, &["artist"]), "");
    }
}
