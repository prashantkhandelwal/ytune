use std::{
    ffi::OsString,
    future::Future,
    process::Stdio,
    sync::mpsc::{self, Receiver},
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::{process::Command, sync::oneshot};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TrackMetadata {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub uploader: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
    pub upload_date: Option<String>,
}

impl TrackMetadata {
    pub fn validate(&self) -> Result<()> {
        if self.title.trim().is_empty() {
            bail!("YouTube returned an empty title");
        }
        if self
            .duration
            .is_some_and(|duration| !duration.is_finite() || duration < 0.0)
        {
            bail!("YouTube returned an invalid duration");
        }
        Ok(())
    }

    pub fn artist_label(&self) -> &str {
        self.artist
            .as_deref()
            .filter(|artist| !artist.trim().is_empty())
            .or(self.uploader.as_deref())
            .unwrap_or("")
    }
}

pub fn executable() -> OsString {
    std::env::var_os("YTUNE_YTDLP").unwrap_or_else(|| "yt-dlp".into())
}

pub type Resolver = Lookup<TrackMetadata>;

pub struct Lookup<T> {
    pub results: Receiver<Result<T, String>>,
    cancel: Option<oneshot::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl<T: Send + 'static> Lookup<T> {
    #[cfg(test)]
    pub fn completed(result: Result<T, String>) -> Self {
        let (sender, results) = mpsc::channel();
        sender.send(result).unwrap();
        Self {
            results,
            cancel: None,
            worker: None,
        }
    }

    pub fn spawn<F, Fut>(name: &'static str, lookup: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>>,
    {
        let (sender, results) = mpsc::channel();
        let (cancel, cancelled) = oneshot::channel();
        let worker = thread::spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .with_context(|| format!("Cannot start {name} lookup"))
                .map(|runtime| {
                    let result = runtime.block_on(async {
                        tokio::select! {
                            _ = cancelled => None,
                            result = tokio::time::timeout(Duration::from_secs(60), lookup()) => {
                                Some(result.with_context(|| format!("{name} lookup timed out after 60 seconds")).and_then(|r| r))
                            }
                        }
                    });
                    // A cancelled HTTP lookup must not wait for blocking system DNS work.
                    runtime.shutdown_background();
                    result
                });
            let result = match result {
                Ok(None) => return,
                Ok(Some(result)) => result,
                Err(error) => Err(error),
            };
            let _ = sender.send(result.map_err(|error| format!("{error:#}")));
        });
        Self {
            results,
            cancel: Some(cancel),
            worker: Some(worker),
        }
    }
}

impl Lookup<TrackMetadata> {
    pub fn start(url: String, executable: OsString) -> Self {
        Self::spawn("Metadata", move || async move {
            resolve(&url, executable).await
        })
    }
}

impl<T> Drop for Lookup<T> {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

async fn resolve(url: &str, executable: OsString) -> Result<TrackMetadata> {
    let output = Command::new(executable)
        .args([
            "--dump-single-json",
            "--skip-download",
            "--no-playlist",
            "--no-progress",
            "--socket-timeout",
            "15",
            "--retries",
            "1",
            "--extractor-retries",
            "1",
            "--",
            url,
        ])
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .context(
            "Cannot run yt-dlp. Set its path in Settings, install it on PATH, or set YTUNE_YTDLP",
        )?;
    if !output.status.success() {
        bail!(
            "yt-dlp metadata lookup failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    parse(&output.stdout)
}

fn parse(json: &[u8]) -> Result<TrackMetadata> {
    let metadata: TrackMetadata =
        serde_json::from_slice(json).context("Invalid yt-dlp metadata")?;
    metadata.validate()?;
    Ok(metadata)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn sample() -> TrackMetadata {
        parse(br#"{"title":"Resolved title","artist":"Artist","album":"Album","uploader":"Channel","duration":120,"thumbnail":"https://example.test/thumb.jpg","upload_date":"20261018"}"#).unwrap()
    }

    #[test]
    fn cancelling_a_lookup_does_not_wait_for_its_timeout() -> Result<()> {
        let (started, ready) = mpsc::channel();
        let lookup = Lookup::spawn("Cancellation test", move || async move {
            started.send(())?;
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(())
        });
        ready.recv_timeout(Duration::from_secs(5))?;
        let start = std::time::Instant::now();
        drop(lookup);
        assert!(start.elapsed() < Duration::from_secs(1));
        Ok(())
    }

    #[test]
    fn parses_metadata_and_rejects_missing_or_empty_titles() -> Result<()> {
        let metadata =
            parse(br#"{"title":"Video","uploader":"Channel","duration":19.5,"formats":[]}"#)?;
        assert_eq!(metadata.title, "Video");
        assert_eq!(metadata.artist_label(), "Channel");
        assert_eq!(metadata.duration, Some(19.5));
        assert!(parse(br#"{"title":""}"#).is_err());
        assert!(parse(br#"{"duration":10}"#).is_err());
        assert!(parse(br#"{"title":"Video","duration":-1}"#).is_err());
        Ok(())
    }

    #[test]
    #[ignore = "Requires yt-dlp and network access; set YTUNE_SMOKE_URL"]
    fn real_metadata_lookup() -> Result<()> {
        let url = std::env::var("YTUNE_SMOKE_URL").context("Set YTUNE_SMOKE_URL")?;
        let resolver = Resolver::start(url, executable());
        let metadata = resolver
            .results
            .recv_timeout(Duration::from_secs(65))?
            .map_err(anyhow::Error::msg)?;
        assert!(!metadata.title.is_empty());
        assert!(metadata.duration.is_some_and(|duration| duration > 0.0));
        assert!(metadata.uploader.is_some());
        println!(
            "Resolved title: {}, duration: {:?}",
            metadata.title, metadata.duration
        );
        Ok(())
    }
}
