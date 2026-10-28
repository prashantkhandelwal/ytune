use std::{
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::library::Library;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct Settings {
    pub ytdlp_path: Option<PathBuf>,
    pub mpv_path: Option<PathBuf>,
    pub seek_seconds: u32,
    pub volume_step: u8,
    pub related_song_limit: usize,
    pub show_thumbnail: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ytdlp_path: None,
            mpv_path: None,
            seek_seconds: 10,
            volume_step: 5,
            related_song_limit: 5,
            show_thumbnail: false,
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<()> {
        if self
            .mpv_path
            .as_ref()
            .is_some_and(|path| path.as_os_str().is_empty())
        {
            bail!("The mpv path cannot be empty; use null for automatic discovery");
        }
        if self
            .ytdlp_path
            .as_ref()
            .is_some_and(|path| path.as_os_str().is_empty())
        {
            bail!("The yt-dlp path cannot be empty; use null for automatic discovery");
        }
        if self.seek_seconds == 0 {
            bail!("Seek interval must be a positive whole number of seconds");
        }
        if !(1..=100).contains(&self.volume_step) {
            bail!("Volume step must be a whole number between 1 and 100");
        }
        if !(1..=100).contains(&self.related_song_limit) {
            bail!("Related song limit must be a whole number between 1 and 100");
        }
        Ok(())
    }

    pub fn executable(&self) -> OsString {
        self.ytdlp_path
            .as_ref()
            .map(|path| path.as_os_str().to_owned())
            .unwrap_or_else(crate::metadata::executable)
    }

    pub fn displayed_executable(&self) -> String {
        Self::display_path(self.executable())
    }

    pub fn mpv_executable(&self) -> OsString {
        self.mpv_path
            .as_ref()
            .map(|path| path.as_os_str().to_owned())
            .unwrap_or_else(|| std::env::var_os("YTUNE_MPV").unwrap_or_else(|| "mpv".into()))
    }

    pub fn displayed_mpv_executable(&self) -> String {
        Self::display_path(self.mpv_executable())
    }

    fn display_path(executable: OsString) -> String {
        match which::which(&executable) {
            Ok(path) => path.to_string_lossy().into_owned(),
            Err(_) => executable.to_string_lossy().into_owned(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Storage {
    pub directory: PathBuf,
    locator_path: PathBuf,
}

#[derive(Deserialize, Serialize)]
struct Location {
    data_dir: PathBuf,
}

impl Storage {
    pub fn at(directory: PathBuf, locator_path: PathBuf) -> Self {
        Self {
            directory,
            locator_path,
        }
    }

    pub fn load(default_directory: &Path) -> Result<(Self, Settings)> {
        let locator_path = default_directory.join("location.json");
        let location: Option<Location> = read_optional(&locator_path)?;
        let relocated = location.is_some();
        let directory = location
            .map(|location| location.data_dir)
            .unwrap_or_else(|| default_directory.to_path_buf());
        if !directory.is_absolute() {
            bail!("The saved data directory must be an absolute path");
        }
        let storage = Self::at(directory, locator_path);
        if relocated && (!storage.library_path().is_file() || !storage.settings_path().is_file()) {
            bail!(
                "Relocated data is missing from {}; restore library.json and settings.json before restarting",
                storage.directory.display()
            );
        }
        let settings: Settings = read_optional(&storage.settings_path())?.unwrap_or_default();
        settings.validate().context("Invalid saved settings")?;
        Ok((storage, settings))
    }

    pub fn library_path(&self) -> PathBuf {
        self.directory.join("library.json")
    }

    pub fn settings_path(&self) -> PathBuf {
        self.directory.join("settings.json")
    }

    pub fn save(&self, directory: &Path, settings: &Settings, library: &Library) -> Result<Self> {
        settings.validate()?;
        if directory.as_os_str().is_empty() {
            bail!("Enter a data folder");
        }
        let directory = std::path::absolute(directory).context("Cannot resolve data folder")?;
        fs::create_dir_all(&directory).context("Cannot create data folder")?;
        let same_directory = if self.directory.try_exists()? {
            fs::canonicalize(&directory)? == fs::canonicalize(&self.directory)?
        } else {
            directory == std::path::absolute(&self.directory)?
        };
        if same_directory {
            write_json(&self.settings_path(), settings, true)?;
            return Ok(self.clone());
        }
        let library_path = directory.join("library.json");
        let settings_path = directory.join("settings.json");
        if library_path.try_exists()? || settings_path.try_exists()? {
            bail!(
                "The destination already contains library.json or settings.json; choose another folder"
            );
        }
        let mut created = Vec::new();
        let result: Result<()> = (|| {
            library.export(&library_path)?;
            created.push(library_path.clone());
            write_json(&settings_path, settings, false)?;
            created.push(settings_path);
            write_json(
                &self.locator_path,
                &Location {
                    data_dir: directory.clone(),
                },
                true,
            )?;
            Ok(())
        })();
        if let Err(error) = result {
            for path in created.iter().rev() {
                fs::remove_file(path).with_context(|| {
                    format!(
                        "Data relocation failed ({error:#}); could not remove newly created {}",
                        path.display()
                    )
                })?;
            }
            return Err(error)
                .context("Data relocation failed; the original location is still active");
        }
        Ok(Self::at(directory, self.locator_path.clone()))
    }
}

fn read_optional<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read_to_string(path) {
        Ok(json) => serde_json::from_str(&json)
            .map(Some)
            .with_context(|| format!("Invalid configuration: {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("Cannot read {}", path.display())),
    }
}

pub fn write_json(path: &Path, value: &impl Serialize, overwrite: bool) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).context("Cannot create configuration directory")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    if overwrite {
        file.persist(path)
            .with_context(|| format!("Cannot save {}", path.display()))?;
    } else {
        file.persist_noclobber(path).with_context(|| {
            format!(
                "Cannot export to {} (existing files are not overwritten)",
                path.display()
            )
        })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_defaults_and_saved_values_survive_reload() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let (storage, settings) = Storage::load(directory.path())?;
        assert_eq!(settings, Settings::default());
        assert!(!settings.show_thumbnail);
        let settings = Settings {
            mpv_path: Some(directory.path().join("mpv folder").join("mpv.exe")),
            seek_seconds: 25,
            volume_step: 7,
            related_song_limit: 20,
            show_thumbnail: true,
            ..settings
        };
        storage.save(directory.path(), &settings, &Library::default())?;
        let (loaded, restored) = Storage::load(directory.path())?;
        assert_eq!(restored, settings);
        assert_eq!(loaded.library_path(), storage.library_path());
        Ok(())
    }

    #[test]
    fn relocation_copies_data_and_remembers_new_location_without_changing_originals() -> Result<()>
    {
        let directory = tempfile::tempdir()?;
        let original = directory.path().join("original");
        let target = directory.path().join("new folder");
        let (storage, settings) = Storage::load(&original)?;
        let mut library = Library::default();
        library.add_resolved(
            "Saved",
            "",
            "https://youtu.be/one",
            crate::metadata::tests::sample(),
        )?;
        library.save(&storage.library_path())?;
        storage.save(&original, &settings, &library)?;
        let original_library = fs::read(storage.library_path())?;
        let original_settings = fs::read(storage.settings_path())?;
        let changed = Settings {
            seek_seconds: 30,
            volume_step: 10,
            ..settings
        };
        let moved = storage.save(&target, &changed, &library)?;
        assert_eq!(moved.directory, target);
        assert_eq!(
            Library::load(&moved.library_path())?.get(0).unwrap().1,
            library.get(0).unwrap().1
        );
        let (loaded, restored) = Storage::load(&original)?;
        assert_eq!(loaded.directory, target);
        assert_eq!(restored, changed);
        assert_eq!(fs::read(storage.library_path())?, original_library);
        assert_eq!(fs::read(storage.settings_path())?, original_settings);
        Ok(())
    }

    #[test]
    fn relocation_refuses_existing_data_and_rolls_back_if_locator_cannot_be_saved() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let original = directory.path().join("original");
        fs::create_dir_all(&original)?;
        let storage = Storage::at(original.clone(), original.join("location.json"));
        let occupied = directory.path().join("occupied");
        fs::create_dir_all(&occupied)?;
        fs::write(occupied.join("library.json"), "keep existing data")?;
        assert!(
            storage
                .save(&occupied, &Settings::default(), &Library::default())
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(occupied.join("library.json"))?,
            "keep existing data"
        );
        fs::create_dir(original.join("location.json"))?;
        let target = directory.path().join("failed migration");
        assert!(
            storage
                .save(&target, &Settings::default(), &Library::default())
                .is_err()
        );
        assert!(!target.join("library.json").exists());
        assert!(!target.join("settings.json").exists());
        assert!(original.join("location.json").is_dir());
        Ok(())
    }

    #[test]
    fn invalid_configuration_and_missing_relocated_data_are_explicit_errors() -> Result<()> {
        let directory = tempfile::tempdir()?;
        fs::write(directory.path().join("settings.json"), "invalid")?;
        assert!(Storage::load(directory.path()).is_err());
        fs::write(
            directory.path().join("settings.json"),
            r#"{"seek_seconds":0}"#,
        )?;
        assert!(Storage::load(directory.path()).is_err());
        fs::write(
            directory.path().join("settings.json"),
            r#"{"volume_step":101}"#,
        )?;
        assert!(Storage::load(directory.path()).is_err());
        write_json(
            &directory.path().join("location.json"),
            &Location {
                data_dir: directory.path().join("missing"),
            },
            true,
        )?;
        assert!(Storage::load(directory.path()).is_err());
        assert!(
            Settings {
                volume_step: 0,
                ..Settings::default()
            }
            .validate()
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn configured_executable_overrides_automatic_discovery_and_is_shown_as_a_full_path()
    -> Result<()> {
        let executable = std::env::current_exe()?;
        let settings = Settings {
            ytdlp_path: Some(executable.clone()),
            mpv_path: Some(executable.clone()),
            ..Settings::default()
        };
        assert_eq!(settings.executable(), executable.as_os_str());
        assert!(Path::new(&settings.displayed_executable()).is_absolute());
        assert_eq!(settings.mpv_executable(), executable.as_os_str());
        assert!(Path::new(&settings.displayed_mpv_executable()).is_absolute());
        Ok(())
    }

    #[test]
    fn older_settings_keep_automatic_mpv_discovery_and_empty_mpv_paths_are_invalid() -> Result<()> {
        let settings: Settings =
            serde_json::from_str(r#"{"ytdlp_path":null,"seek_seconds":25,"volume_step":7}"#)?;
        assert_eq!(settings.mpv_path, None);
        assert_eq!(settings.related_song_limit, 5);
        assert!(!settings.show_thumbnail);
        assert_eq!(settings.seek_seconds, 25);
        assert_eq!(
            settings.mpv_executable(),
            std::env::var_os("YTUNE_MPV").unwrap_or_else(|| "mpv".into())
        );
        assert!(
            Settings {
                mpv_path: Some(PathBuf::new()),
                ..settings
            }
            .validate()
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn related_song_limit_accepts_only_one_to_one_hundred() {
        for limit in [0, 1, 5, 100, 101] {
            let settings = Settings {
                related_song_limit: limit,
                ..Settings::default()
            };
            assert_eq!(settings.validate().is_ok(), (1..=100).contains(&limit));
        }
    }

    #[test]
    fn thumbnail_visibility_requires_a_json_boolean() {
        for value in [r#""false""#, "null", "0"] {
            assert!(
                serde_json::from_str::<Settings>(&format!(r#"{{"show_thumbnail":{value}}}"#))
                    .is_err()
            );
        }
    }
}
