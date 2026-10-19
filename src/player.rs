use std::{
    ffi::{OsStr, OsString},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

#[cfg(windows)]
type Connection = tokio::net::windows::named_pipe::NamedPipeClient;
#[cfg(unix)]
type Connection = tokio::net::UnixStream;

pub enum PlayerEvent {
    Ready,
    Message(Value),
    Error(String),
    YtdlpMissing(String),
}

#[derive(Debug)]
struct MissingYtdlp;

impl std::fmt::Display for MissingYtdlp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "yt-dlp was not found at the configured path or on PATH.\n\nPress t to set its path in Settings. You can also install yt-dlp or set YTUNE_YTDLP, then save Settings or restart ytune.",
        )
    }
}

impl std::error::Error for MissingYtdlp {}

pub struct Player {
    commands: Sender<Value>,
    pub events: Receiver<PlayerEvent>,
    worker: Option<JoinHandle<()>>,
}

impl Player {
    #[cfg(test)]
    pub fn test_channels() -> (Self, Receiver<Value>, Sender<PlayerEvent>) {
        let (commands, incoming) = mpsc::channel();
        let (outgoing, events) = mpsc::channel();
        (
            Self {
                commands,
                events,
                worker: None,
            },
            incoming,
            outgoing,
        )
    }

    pub fn start(executable: OsString, mpv: OsString) -> Self {
        let (commands, incoming) = mpsc::channel();
        let (outgoing, events) = mpsc::channel();
        let worker = thread::spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("Cannot start playback runtime")
                .and_then(|runtime| runtime.block_on(run(incoming, &outgoing, executable, mpv)));
            if let Err(error) = result {
                let event = if error.downcast_ref::<MissingYtdlp>().is_some() {
                    PlayerEvent::YtdlpMissing(format!("{error:#}"))
                } else {
                    PlayerEvent::Error(format!("{error:#}"))
                };
                let _ = outgoing.send(event);
            }
        });
        Self {
            commands,
            events,
            worker: Some(worker),
        }
    }

    pub fn command(&self, command: Value) -> Result<()> {
        self.commands
            .send(command)
            .context("Playback engine is unavailable. Check dependencies and restart ytune")
    }

    pub fn shutdown(&mut self) {
        let _ = self.commands.send(json!(["quit"]));
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct Engine {
    child: Child,
    endpoint: String,
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Killing only our child also unblocks the IPC reader on shutdown.
        let _ = self.child.kill();
        let _ = self.child.wait();
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.endpoint);
    }
}

fn check_ytdlp(executable: &OsStr) -> Result<()> {
    let check = Command::new(executable)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                anyhow::Error::new(MissingYtdlp)
            } else {
                anyhow::Error::new(error).context("Cannot run yt-dlp --version")
            }
        })?;
    if !check.success() {
        bail!("yt-dlp --version failed; check your yt-dlp installation");
    }
    Ok(())
}

async fn run(
    commands: Receiver<Value>,
    events: &Sender<PlayerEvent>,
    ytdlp: OsString,
    mpv: OsString,
) -> Result<()> {
    check_ytdlp(&ytdlp)?;
    #[cfg(windows)]
    let endpoint = format!(r"\\.\pipe\ytune-{}", std::process::id());
    #[cfg(unix)]
    let endpoint = std::env::temp_dir()
        .join(format!("ytune-{}.sock", std::process::id()))
        .to_string_lossy()
        .into_owned();
    let child = Command::new(mpv)
        .args([
            "--no-config",
            "--idle=yes",
            "--no-video",
            "--audio-display=no",
            "--terminal=no",
            "--ytdl=yes",
            "--ytdl-format=bestaudio/best",
            "--volume=70",
        ])
        .arg(format!("--input-ipc-server={endpoint}"))
        .arg(format!(
            "--script-opts=ytdl_hook-ytdl_path={}",
            ytdlp.to_string_lossy()
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context(
            "Cannot start mpv. Set its executable path in Settings, install mpv, or set YTUNE_MPV",
        )?;
    let mut engine = Engine { child, endpoint };
    let connection = connect(&mut engine).await?;
    let (reader, mut writer) = tokio::io::split(connection);
    let sender = events.clone();
    let reader = tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Some(line) = lines.next_line().await.context("mpv IPC read failed")? {
            let value = serde_json::from_str(&line).context("Invalid mpv response")?;
            if sender.send(PlayerEvent::Message(value)).is_err() {
                return Ok::<(), anyhow::Error>(());
            }
        }
        bail!("mpv closed its IPC connection");
    });
    for (id, property) in [
        "media-title",
        "duration",
        "time-pos",
        "pause",
        "volume",
        "mute",
        "metadata",
    ]
    .iter()
    .enumerate()
    {
        send(&mut writer, json!(["observe_property", id, property])).await?;
    }
    send(&mut writer, json!(["request_log_messages", "error"])).await?;
    let _ = events.send(PlayerEvent::Ready);
    loop {
        match commands.try_recv() {
            Ok(command) => {
                let quit = command[0] == "quit";
                send(&mut writer, command).await?;
                if quit {
                    break;
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                send(&mut writer, json!(["quit"])).await?;
                break;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if reader.is_finished() {
            reader.await.context("mpv IPC reader task failed")??;
            bail!("mpv IPC reader stopped unexpectedly");
        }
        if let Some(status) = engine.child.try_wait()? {
            bail!("mpv exited unexpectedly ({status})");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // Give mpv time to cancel extraction subprocesses before the kill fallback.
    let deadline = Instant::now() + Duration::from_secs(2);
    while engine.child.try_wait()?.is_none() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(())
}

async fn connect(engine: &mut Engine) -> Result<Connection> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        #[cfg(windows)]
        let result = tokio::net::windows::named_pipe::ClientOptions::new().open(&engine.endpoint);
        #[cfg(unix)]
        let result = Connection::connect(&engine.endpoint).await;
        match result {
            Ok(connection) => return Ok(connection),
            Err(error) if Instant::now() >= deadline => {
                return Err(error).context("Timed out connecting to mpv");
            }
            Err(_) => {
                if let Some(status) = engine.child.try_wait()? {
                    bail!("mpv exited before its IPC server was ready ({status})");
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

async fn send(connection: &mut (impl AsyncWrite + Unpin), command: Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(&json!({ "command": command }))?;
    bytes.push(b'\n');
    tokio::time::timeout(Duration::from_secs(3), connection.write_all(&bytes))
        .await
        .context("Timed out writing to mpv")?
        .context("Cannot send command to mpv")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn nonexistent_ytdlp_is_classified_as_a_missing_dependency() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let executable = directory.path().join("missing-yt-dlp.exe");
        let error = check_ytdlp(executable.as_os_str()).unwrap_err();
        assert!(error.downcast_ref::<MissingYtdlp>().is_some());
        let message = error.to_string();
        assert!(message.contains("PATH"));
        assert!(message.contains("YTUNE_YTDLP"));
        assert!(message.contains("restart ytune"));
        Ok(())
    }

    fn wait_for(player: &Player, predicate: impl Fn(&Value) -> bool) -> Result<Value> {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            match player.events.recv_timeout(Duration::from_millis(500)) {
                Ok(PlayerEvent::Message(message)) => {
                    if message["event"] == "end-file" && message["reason"] == "error" {
                        bail!("mpv playback failed: {message}");
                    }
                    if predicate(&message) {
                        return Ok(message);
                    }
                }
                Ok(PlayerEvent::Error(error) | PlayerEvent::YtdlpMissing(error)) => {
                    bail!("{error}")
                }
                Ok(PlayerEvent::Ready) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => return Err(error.into()),
            }
        }
        bail!("Timed out waiting for mpv playback event");
    }

    #[test]
    #[ignore = "Requires mpv, yt-dlp and an audio device; optionally set YTUNE_SMOKE_URL"]
    fn real_mpv_playback_smoke_test() -> Result<()> {
        let player = Player::start(
            crate::metadata::executable(),
            crate::config::Settings::default().mpv_executable(),
        );
        loop {
            match player.events.recv_timeout(Duration::from_secs(15))? {
                PlayerEvent::Ready => break,
                PlayerEvent::Error(error) | PlayerEvent::YtdlpMissing(error) => bail!("{error}"),
                PlayerEvent::Message(_) => {}
            }
        }
        let mut audio = tempfile::Builder::new().suffix(".wav").tempfile()?;
        let samples = 8000_u32 * 3;
        let mut header = Vec::new();
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&(36 + samples * 2).to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&16_u32.to_le_bytes());
        header.extend_from_slice(&1_u16.to_le_bytes());
        header.extend_from_slice(&1_u16.to_le_bytes());
        header.extend_from_slice(&8000_u32.to_le_bytes());
        header.extend_from_slice(&16000_u32.to_le_bytes());
        header.extend_from_slice(&2_u16.to_le_bytes());
        header.extend_from_slice(&16_u16.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&(samples * 2).to_le_bytes());
        audio.write_all(&header)?;
        audio.write_all(&vec![0; samples as usize * 2])?;
        audio.flush()?;

        player.command(json!([
            "loadfile",
            audio.path().to_string_lossy(),
            "replace"
        ]))?;
        wait_for(&player, |m| m["event"] == "file-loaded").context("Loading local WAV")?;
        player.command(json!(["set_property", "pause", true]))?;
        wait_for(&player, |m| m["name"] == "pause" && m["data"] == true)
            .context("Pausing playback")?;
        player.command(json!(["set_property", "volume", 35]))?;
        wait_for(&player, |m| {
            m["name"] == "volume" && m["data"].as_f64() == Some(35.0)
        })
        .context("Changing volume")?;
        player.command(json!(["seek", 1, "absolute"]))?;
        player.command(json!(["set_property", "pause", false]))?;
        wait_for(&player, |m| {
            m["event"] == "end-file" && m["reason"] == "eof"
        })
        .context("Seeking, resuming, and reaching EOF")?;
        println!("Verified real mpv load, pause, volume, seek, resume, and EOF");

        if let Ok(url) = std::env::var("YTUNE_SMOKE_URL") {
            player.command(json!(["loadfile", url, "replace"]))?;
            wait_for(&player, |m| m["event"] == "file-loaded").context("Loading YouTube audio")?;
            let position = wait_for(&player, |m| {
                m["name"] == "time-pos" && m["data"].as_f64().is_some_and(|p| p >= 0.5)
            })
            .context("Checking YouTube playback progress")?;
            println!("Verified YouTube streaming and advancing playback: {position}");
        }
        let shutdown = Instant::now();
        drop(player);
        assert!(
            shutdown.elapsed() < Duration::from_secs(3),
            "Player shutdown exceeded 3 seconds"
        );
        Ok(())
    }
}
