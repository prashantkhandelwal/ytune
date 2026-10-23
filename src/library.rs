use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::metadata::TrackMetadata;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Track {
    pub title: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<TrackMetadata>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Playlist {
    pub name: String,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Library {
    pub playlists: Vec<Playlist>,
}

#[derive(Debug, Default, PartialEq)]
pub struct ImportSummary {
    pub added: usize,
    pub skipped: usize,
}

impl Library {
    pub fn path() -> Result<PathBuf> {
        let dirs = ProjectDirs::from("com", "ytune", "ytune")
            .context("Could not determine the configuration directory")?;
        Ok(dirs.config_dir().join("library.json"))
    }

    pub fn load(path: &PathBuf) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(json) => serde_json::from_str(&json)
                .with_context(|| format!("Invalid library: {}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("Cannot read {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.write_json(path, true)
    }

    pub fn export(&self, path: &Path) -> Result<()> {
        self.write_json(path, false)
    }

    pub fn merge_file(&self, path: &Path) -> Result<(Self, ImportSummary)> {
        let json = fs::read_to_string(path)
            .with_context(|| format!("Cannot read import file {}", path.display()))?;
        let imported: Library =
            serde_json::from_str(json.strip_prefix('\u{feff}').unwrap_or(&json))
                .with_context(|| format!("Invalid playlist JSON: {}", path.display()))?;
        let mut merged = self.clone();
        let mut summary = ImportSummary::default();
        for playlist in imported.playlists {
            let name = playlist.name.trim();
            if name.is_empty() || name.chars().any(char::is_control) {
                bail!("Imported category names must be nonempty and contain no control characters");
            }
            for track in playlist.tracks {
                let url = track.url.trim();
                validate_url(url)
                    .with_context(|| format!("Invalid track URL in category {name}: {url}"))?;
                if track.url.chars().any(char::is_control)
                    || track.title.chars().any(char::is_control)
                {
                    bail!(
                        "Imported track title and URL must contain no control characters (category {name})"
                    );
                }
                if let Some(metadata) = &track.metadata {
                    metadata.validate().with_context(|| {
                        format!("Invalid metadata for {} in category {name}", track.title)
                    })?;
                }
                if merged.index_of(name, url).is_some() {
                    summary.skipped += 1;
                    continue;
                }
                match track.metadata {
                    Some(metadata) => {
                        merged.add_resolved(name, &track.title, url, metadata)?;
                    }
                    None => {
                        merged.add(name, &track.title, url)?;
                    }
                }
                summary.added += 1;
            }
        }
        Ok((merged, summary))
    }

    fn write_json(&self, path: &Path, overwrite: bool) -> Result<()> {
        crate::config::write_json(path, self, overwrite)
    }

    pub fn len(&self) -> usize {
        self.playlists.iter().map(|p| p.tracks.len()).sum()
    }

    pub fn get(&self, index: usize) -> Option<(&str, &Track)> {
        self.playlists
            .iter()
            .flat_map(|p| p.tracks.iter().map(move |track| (p.name.as_str(), track)))
            .nth(index)
    }

    pub fn index_of(&self, category: &str, url: &str) -> Option<usize> {
        self.playlists
            .iter()
            .flat_map(|playlist| {
                playlist
                    .tracks
                    .iter()
                    .map(move |track| (playlist.name.as_str(), track))
            })
            .position(|(name, track)| name == category && track.url == url)
    }

    pub fn add_resolved(
        &mut self,
        category: &str,
        title: &str,
        url: &str,
        metadata: TrackMetadata,
    ) -> Result<usize> {
        let title = if title.trim().is_empty() {
            metadata.title.as_str()
        } else {
            title
        };
        let index = self.add(category, title, url)?;
        self.playlists
            .iter_mut()
            .flat_map(|p| &mut p.tracks)
            .nth(index)
            .context("Added track could not be located")?
            .metadata = Some(metadata);
        Ok(index)
    }

    pub fn add(&mut self, category: &str, title: &str, url: &str) -> Result<usize> {
        let category = category.trim();
        let url = url.trim();
        if category.is_empty() {
            bail!("Enter a playlist or category name");
        }
        validate_url(url)?;
        if self.index_of(category, url).is_some() {
            bail!("That URL is already in this category");
        }
        let track = Track {
            title: if title.trim().is_empty() {
                url.to_string()
            } else {
                title.trim().to_string()
            },
            url: url.to_string(),
            metadata: None,
        };
        if let Some(playlist) = self.playlists.iter_mut().find(|p| p.name == category) {
            playlist.tracks.push(track);
        } else {
            self.playlists.push(Playlist {
                name: category.to_string(),
                tracks: vec![track],
            });
        }
        self.index_of(category, url)
            .context("Added track could not be located")
    }

    pub fn remove(&mut self, index: usize) {
        let mut remaining = index;
        for playlist in &mut self.playlists {
            if remaining < playlist.tracks.len() {
                playlist.tracks.remove(remaining);
                break;
            }
            remaining -= playlist.tracks.len();
        }
        self.playlists.retain(|p| !p.tracks.is_empty());
    }

    pub fn edit(&mut self, index: usize, fields: &[String; 4]) -> Result<usize> {
        let [title, category, artist, album] = fields.each_ref().map(|value| value.trim());
        if title.is_empty() || category.is_empty() {
            bail!("Song name and category must not be empty");
        }
        if fields
            .iter()
            .any(|value| value.chars().any(char::is_control))
        {
            bail!("Audio details must contain no control characters");
        }
        let (original_category, original) = self.get(index).context("Track no longer exists")?;
        if self
            .index_of(category, &original.url)
            .is_some_and(|other| other != index)
        {
            bail!("That URL is already in this category");
        }
        let same_category = original_category == category;
        let mut track = original.clone();
        track.title = title.into();
        let metadata = track.metadata.get_or_insert_with(|| TrackMetadata {
            title: title.into(),
            artist: None,
            album: None,
            uploader: None,
            duration: None,
            thumbnail: None,
            upload_date: None,
        });
        metadata.artist = Some(artist.into());
        metadata.album = Some(album.into());
        if same_category {
            *self
                .playlists
                .iter_mut()
                .flat_map(|playlist| &mut playlist.tracks)
                .nth(index)
                .context("Track no longer exists")? = track;
            return Ok(index);
        }
        let url = track.url.clone();
        self.remove(index);
        if let Some(playlist) = self.playlists.iter_mut().find(|p| p.name == category) {
            playlist.tracks.push(track);
        } else {
            self.playlists.push(Playlist {
                name: category.into(),
                tracks: vec![track],
            });
        }
        self.index_of(category, &url)
            .context("Edited track could not be located")
    }
}

fn validate_url(value: &str) -> Result<()> {
    let url = Url::parse(value).context("Enter a complete YouTube video URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("Use an http(s) YouTube video URL without credentials");
    }
    let host = url.host_str().unwrap_or_default();
    let valid = match host {
        "youtu.be" => url
            .path_segments()
            .is_some_and(|mut s| s.next().is_some_and(|id| !id.is_empty())),
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com" => {
            (url.path() == "/watch"
                && url
                    .query_pairs()
                    .any(|(key, value)| key == "v" && !value.is_empty()))
                || ["/shorts/", "/live/", "/embed/"].iter().any(|prefix| {
                    url.path()
                        .strip_prefix(prefix)
                        .is_some_and(|id| !id.is_empty())
                })
        }
        _ => false,
    };
    if !valid {
        bail!("Use a YouTube video URL, not a channel or playlist-only URL");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_tracks_and_removes_empty_categories() -> Result<()> {
        let mut library = Library::default();
        library.add("Rock", "One", "https://youtu.be/one")?;
        library.add("Jazz", "Two", "https://youtu.be/two")?;
        assert_eq!(library.add("Rock", "Three", "https://youtu.be/three")?, 1);
        assert_eq!(library.get(2).unwrap().0, "Jazz");
        library.remove(2);
        assert_eq!(library.playlists.len(), 1);
        assert!(
            library
                .add("Rock", "Duplicate", "https://youtu.be/one")
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn accepts_only_youtube_video_urls() {
        for url in [
            "https://youtu.be/abc",
            "https://www.youtube.com/watch?v=abc&list=xyz",
            "https://music.youtube.com/watch?v=abc",
            "https://youtube.com/shorts/abc",
        ] {
            assert!(validate_url(url).is_ok(), "{url}");
        }
        for url in [
            "file:///etc/passwd",
            "https://youtube.com.evil.test/watch?v=abc",
            "https://youtube.com/playlist?list=abc",
            "https://youtu.be/",
            "https://user:pass@youtube.com/watch?v=abc",
        ] {
            assert!(validate_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn saves_and_replaces_library_without_losing_data() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("library.json");
        let mut library = Library::load(&path)?;
        library.add("Test", "First", "https://youtu.be/first")?;
        library.save(&path)?;
        library.add("Test", "Second", "https://youtu.be/second")?;
        library.save(&path)?;
        assert_eq!(Library::load(&path)?.len(), 2);
        fs::write(&path, "invalid json")?;
        assert!(Library::load(&path).is_err());
        Ok(())
    }

    #[test]
    fn imports_merge_categories_skip_duplicates_and_keep_metadata() -> Result<()> {
        let mut library = Library::default();
        library.add("First", "Keep existing title", "https://youtu.be/one")?;
        let mut source = Library::default();
        source.add_resolved(
            "First",
            "Duplicate",
            "https://youtu.be/one",
            crate::metadata::tests::sample(),
        )?;
        source.add_resolved(
            "First",
            "",
            "https://youtu.be/new",
            crate::metadata::tests::sample(),
        )?;
        source.add_resolved(
            "Second",
            "",
            "https://youtu.be/one",
            crate::metadata::tests::sample(),
        )?;
        let duplicate = source.playlists[0].tracks[1].clone();
        source.playlists[0].tracks.push(duplicate);
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("source.json");
        source.export(&path)?;
        let source_bytes = fs::read(&path)?;
        let (merged, summary) = library.merge_file(&path)?;
        assert_eq!(
            summary,
            ImportSummary {
                added: 2,
                skipped: 2
            }
        );
        assert_eq!(merged.playlists.len(), 2);
        assert_eq!(merged.get(0).unwrap().1.title, "Keep existing title");
        assert!(merged.get(0).unwrap().1.metadata.is_none());
        assert_eq!(
            merged.get(1).unwrap().1.metadata,
            Some(crate::metadata::tests::sample())
        );
        assert_eq!(merged.get(2).unwrap().0, "Second");
        assert_eq!(library.len(), 1);
        assert_eq!(fs::read(&path)?, source_bytes);
        let (_, repeated) = merged.merge_file(&path)?;
        assert_eq!(
            repeated,
            ImportSummary {
                added: 0,
                skipped: 4
            }
        );
        Ok(())
    }

    #[test]
    fn invalid_imports_are_atomic_and_missing_files_are_errors() -> Result<()> {
        let mut library = Library::default();
        library.add("Existing", "Keep", "https://youtu.be/keep")?;
        let before = serde_json::to_value(&library)?;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("source.json");
        assert!(library.merge_file(&path).is_err());
        assert!(library.merge_file(directory.path()).is_err());
        for invalid in [
            "{",
            r#"{"playlists":[{"name":"New","tracks":[{"title":"Valid","url":"https://youtu.be/valid"},{"title":"Bad","url":"https://example.test/bad"}]}]}"#,
            r#"{"playlists":[{"name":" ","tracks":[]}]}"#,
            r#"{"playlists":[{"name":"New","tracks":[{"title":"Bad","url":"https://youtu.be/bad","metadata":{"title":"Bad","duration":-1}}]}]}"#,
            r#"{"playlists":[{"name":"New","tracks":[{"title":"Bad\nTitle","url":"https://youtu.be/bad"}]}]}"#,
        ] {
            fs::write(&path, invalid)?;
            assert!(library.merge_file(&path).is_err(), "{invalid}");
            assert_eq!(serde_json::to_value(&library)?, before);
        }
        fs::write(&path, "\u{feff}{\"playlists\":[]}")?;
        let (merged, summary) = library.merge_file(&path)?;
        assert_eq!(summary, ImportSummary::default());
        assert_eq!(serde_json::to_value(&merged)?, before);
        Ok(())
    }

    #[test]
    fn old_libraries_still_load_and_exports_preserve_metadata_without_overwriting() -> Result<()> {
        let old: Library = serde_json::from_str(
            r#"{"playlists":[{"name":"Old","tracks":[{"title":"Old track","url":"https://youtu.be/old"}]}]}"#,
        )?;
        assert!(old.get(0).unwrap().1.metadata.is_none());
        let mut library = old;
        library.add_resolved(
            "New",
            "",
            "https://youtu.be/new",
            crate::metadata::tests::sample(),
        )?;
        library.add_resolved(
            "New",
            "Custom title",
            "https://youtu.be/custom",
            crate::metadata::tests::sample(),
        )?;
        assert_eq!(library.get(1).unwrap().1.title, "Resolved title");
        assert_eq!(library.get(2).unwrap().1.title, "Custom title");
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("export.json");
        library.export(&path)?;
        let before = fs::read(&path)?;
        assert!(library.export(&path).is_err());
        assert_eq!(fs::read(&path)?, before);
        let exported = Library::load(&path)?;
        assert_eq!(exported.len(), 3);
        assert_eq!(
            exported.get(1).unwrap().1.metadata,
            Some(crate::metadata::tests::sample())
        );
        Ok(())
    }
}
