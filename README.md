# ytune

A keyboard-driven YouTube audio player for your terminal, built with Rust and
Ratatui. Organize video URLs into categories, stream audio through mpv, and
discover more recordings by an artist without leaving the app.

## Contents

- [Features](#features)
- [Requirements](#requirements)
- [Installation](#installation)
- [Usage and keyboard shortcuts](#usage)
- [Related songs](#related-songs)
- [Playback behavior](#playback-behavior)
- [Export](#export)
- [Import](#import)
- [Settings](#settings)
- [Saved library](#saved-library)
- [Troubleshooting](#troubleshooting)
- [Development](#development)
- [License](#license)

## Features

- **Audio-only streaming:** Play individual YouTube videos, Shorts, live videos,
  and `youtu.be` links through mpv and yt-dlp without a video window.
- **Playback controls:** Play, pause, resume, stop, seek, adjust volume, and mute
  without losing the volume level.
- **Queue and repeat:** Navigate across all categories, including collapsed ones,
  with automatic track progression and Off, Track, and All repeat modes.
- **Live audio details:** View playback state, elapsed/total time, a seek bar,
  source URL, artist, album, category, volume, and repeat status.
- **Track thumbnails:** Show the stored YouTube thumbnail at the top-right of
  Audio Details using native terminal images where supported, with colored blocks
  as a fallback.
- **Organized library:** Group tracks into expandable categories, prevent duplicate
  URLs within a category, and confirm track deletions.
- **Saved metadata:** Fetch and retain source titles, artists, albums, uploaders,
  durations, thumbnail URLs, and upload dates when available.
- **Track editing:** Customize display titles and edit names, categories, artists,
  and albums without a network lookup or interrupting playback.
- **Reliable local storage:** Save library changes immediately and atomically,
  preserving the previous library if a save fails.
- **JSON import and export:** Export the complete library with metadata without
  overwriting existing files; merge imports with validation, duplicate skipping,
  and an added/skipped summary, including older libraries without metadata.
- **Artist discovery:** Browse related MusicBrainz recordings, choose credited
  artists, disambiguate same-name artists, and paginate 1-100 distinct songs per
  artist per page.
- **YouTube discovery search:** Browse up to five videos for a related recording,
  then save a result to a category or save and play it.
- **Responsive lookups:** Keep navigating during cancellable background searches,
  reuse session-cached discovery results, and refresh or retry lookups.
- **Keyboard-driven interface:** Use a two-pane terminal layout, scrolling lists,
  category counts, distinct playback indicators, context-sensitive shortcuts,
  and an in-app shortcut dialog.
- **Configurable settings:** Change seek intervals, volume steps, discovery page
  sizes, executable paths, and the data folder while retaining original files.
- **Offline library management:** Organize saved tracks without internet access;
  adding tracks and streaming require YouTube, while discovery also uses MusicBrainz.
- **Cross-platform playback:** Control mpv through Windows named pipes or
  Linux/macOS Unix sockets, stopping only ytune's own mpv child on exit.

**Scope:** ytune streams individual videos. It does not import YouTube playlists,
download audio for offline playback, show video, or provide account/cookie setup.
Categories are local library groups, not synchronized YouTube playlists.

## Requirements

- Current stable Rust and Cargo (the project uses Rust edition 2024), plus the
  platform build tools below, to build and install ytune
- Git to clone the repository
- [mpv](https://mpv.io/installation/) with its standard yt-dlp hook
- A current [yt-dlp](https://github.com/yt-dlp/yt-dlp#installation) executable
- A [yt-dlp-supported JavaScript runtime](https://github.com/yt-dlp/yt-dlp/wiki/EJS)
  (Deno is enabled by default), recommended for full YouTube format support
- Internet access and an audio output device
- A terminal at least 64 columns by 20 rows

Rust, Cargo, Git, and the compiler/linker are needed for installation from source,
not for running an already-built executable. mpv, yt-dlp, and the JavaScript
runtime must remain installed for playback.

## Installation

The steps below install ytune from this repository. Run the commands in a terminal;
no YouTube API key or MusicBrainz account is required.

### 1. Install the dependencies

#### Windows (PowerShell)

1. Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
   with the **Desktop development with C++** workload, including the MSVC tools
   and Windows SDK. Rust's default Windows toolchain needs these to link the app.
2. Install Git, Rustup, yt-dlp, and Deno:

```powershell
winget install --id Git.Git -e
winget install --id Rustlang.Rustup -e
winget install --id yt-dlp.yt-dlp -e
winget install --id DenoLand.Deno -e
```

3. Install a Windows mpv build linked from the
   [mpv installation page](https://mpv.io/installation/). Extract the complete
   package to a permanent folder, such as `C:\tools\mpv`, and add that folder to
   your user PATH. The package must contain **mpv.exe itself**, not just a GUI
   launcher, and include its standard yt-dlp hook.
4. Close and reopen PowerShell so PATH changes take effect, then select stable Rust:

```powershell
rustup default stable
rustup update stable
```

If you prefer not to add mpv to PATH, set its full path before launching ytune:

```powershell
$env:YTUNE_MPV = 'C:\tools\mpv\mpv.exe'
```

This environment override applies to the current PowerShell session. You can
save a permanent executable path in ytune's Settings after installation.
Paths containing spaces are supported; do not include literal quote characters
in paths entered into Settings.

#### macOS (Terminal)

Install Apple's command-line build tools:

```sh
xcode-select --install
```

Install [Homebrew](https://brew.sh/) if needed, then install playback dependencies:

```sh
brew install mpv yt-dlp deno
```

Install Rust using [Rustup](https://rustup.rs/). Follow its prompts, reopen the
terminal, then run:

```sh
rustup default stable
rustup update stable
```

The Apple command-line tools also provide Git.

#### Linux (Ubuntu/Debian example)

Install Git, build tools, and mpv:

```sh
sudo apt update
sudo apt install git build-essential curl ca-certificates mpv
```

Install current stable Rust using [Rustup](https://rustup.rs/), and install
yt-dlp and Deno using their official instructions:

- [yt-dlp installation](https://github.com/yt-dlp/yt-dlp#installation)
- [Deno installation](https://docs.deno.com/runtime/getting_started/installation/)

Reopen the terminal after installation, then run:

```sh
rustup default stable
rustup update stable
```

On other distributions, use the equivalent compiler/linker, Git, and mpv
packages. Distribution versions of yt-dlp can lag behind YouTube changes; use a
current upstream release if extraction fails.

### 2. Verify the tools

In the terminal you will use to install and run ytune:

```text
git --version
rustc --version
cargo --version
mpv --version
yt-dlp --version
deno --version
```

Every command should print a version. If using `YTUNE_MPV` instead of PATH on
Windows, replace the mpv check with `& $env:YTUNE_MPV --version`. Deno is the
recommended JavaScript runtime here; another runtime must be configured according
to [yt-dlp's EJS documentation](https://github.com/yt-dlp/yt-dlp/wiki/EJS).

### 3. Install ytune

Run these commands on any of the platforms above:

```text
git clone https://github.com/prashantkhandelwal/ytune.git
cd ytune
cargo install --path . --locked
ytune
```

Cargo builds an optimized executable and installs it in its binary directory.
If `ytune` is not found, reopen the terminal and ensure Cargo's binary directory
is on PATH: `%USERPROFILE%\.cargo\bin` on Windows or `$HOME/.cargo/bin` on
Linux/macOS by default (a custom `CARGO_HOME` changes this location).

Alternatively, build and run without installing:

```text
cargo run --release --locked
```

Run that alternative from the cloned repository. After a release build, the
standalone app is `target\release\ytune.exe` on Windows or `target/release/ytune`
on Linux/macOS. Run it in an interactive terminal, not by double-clicking it.

### 4. Play your first track

1. Launch `ytune` in a terminal at least **64 columns by 20 rows**.
2. Wait for the playback engine to become ready. If the header says **Not Ready**,
   press **t**, set the mpv/yt-dlp executable paths, and press **Enter** to save
   and retry startup. See [Troubleshooting](#troubleshooting) if it remains unavailable.
3. Press **a**. The URL field is focused; paste an individual YouTube video URL.
4. Use **Tab / Shift+Tab** to choose a category (default `Favorites`) and optionally
   enter a display title. Press **Enter** and wait for metadata lookup to complete.
5. Select the saved track with **Up / Down**, then press **Space** to stream audio.
6. Press **k** for playback shortcuts, **t** for Settings, or **q** to quit from
   the main view. Changes to the library are saved automatically.

### Updating

With a clean checkout, run the following from the cloned repository:

```text
git pull --ff-only
cargo install --path . --locked --force
```

If running without installing, rebuild with `cargo build --release --locked`
instead. Keep yt-dlp, mpv, and your JavaScript runtime up to date using the method
used to install them. Updating the executable does not replace the library or
settings stored in the configured data folder.

## Usage

1. Run `ytune` (or `cargo run --release --locked` from the repository).
2. Press **a**, paste a YouTube video URL, and use **Tab** to edit its category
   and optional display-title override. Press **Enter** to resolve the video's
   title and metadata and save it. The UI stays responsive during lookup;
   **Esc** cancels without adding the track.
3. Select a track and press **Space** to stream its audio.

Playback selections (**Space**, **n**, and **b**) wait 400 ms before resolving
YouTube audio and metadata. Rapid changes reset this delay, so only the final
track starts a network lookup. The list and saved details update immediately;
the previous audio may continue briefly until the new track starts loading.
**s** cancels a pending selection. Automatic queue progression does not wait.

Use a normal video, Shorts, live-video, or `youtu.be` URL. A video URL containing
a playlist parameter plays that video only. Playlist-only URLs are not
imported: create a named category and add its videos individually.
The queue follows the displayed category/track order, across categories.
New tracks store the resolved source title, artist, album, uploader, duration,
thumbnail URL, and upload date when available. An optional custom display title
overrides the resolved title in the list without discarding the source title.
Saved metadata is available in the details pane before playback. Existing
tracks remain compatible but are not automatically backfilled.

Select a category header with **Up/Down** and press **Tab** or **Space** to
collapse/expand it. **Tab** also works while one of its tracks is selected.
`[+]` indicates a collapsed category and `[-]` an expanded one. Hidden tracks
remain in the playback queue; next/previous and repeat still traverse the
entire library. Collapse state lasts for the current session.

| Key | Action |
| --- | --- |
| Up / Down (or j for down) | Select previous / next visible track or category |
| k | Open / close the player shortcuts dialog |
| Home / End | Select first / last visible row |
| Space | Play/pause selected track; toggle selected category |
| Enter | Edit the selected track's song name, category, artist, and album |
| Tab | Expand / collapse the selected category |
| n / b | Play next / previous track (wrap around) |
| r | Cycle repeat: Off, Track, All |
| Left / Right | Seek backward / forward by the configured interval (default 10 seconds) |
| + (or =) / - | Adjust volume by the configured step (default 5%; volume stays within 0–100%) |
| a | Add a URL to a new or existing category |
| m | Toggle mute / unmute without changing the volume level |
| e | Export the entire library as JSON |
| i | Import and merge a saved JSON playlist/library |
| t | Open Settings |
| d | Browse related MusicBrainz recordings in Audio Details |
| f | Refresh related songs; retry the lookup in a discovery view |
| s | Stop audio and reset playback position |
| Delete | Ask to remove the selected track; y confirms, n / Esc cancels |
| q | Quit (main view) |

Press **k** to open the player shortcuts dialog, and **k** or **Esc** to close it.
Track selection, play/pause, next/previous, seeking, volume, repeat, mute, stop,
and quit remain available while it is open; close it before using library or
Settings commands. The footer changes to show shortcuts for the active dialog
or discovery view.

In the add dialog, **Tab / Shift+Tab** changes fields, **Ctrl+U** clears a
field, **Backspace** deletes the last character, **Enter** resolves metadata
and saves, and **Esc** cancels. Text is appended at the end of the active field.
Fields are locked while metadata is resolving. Failed lookups or saves leave
the dialog open with an error and do not add a partial track. Lookup times out
after 60 seconds; fix the problem and press Enter to retry.

Select a track in the library and press **Enter** to edit its saved audio
details. **Tab / Shift+Tab** changes fields, **Ctrl+U** clears a field,
**Backspace** deletes the last character, **Enter** saves, and **Esc** cancels.
Changing the category moves the track to that category (creating it if needed).
Song name and category are required; duplicate URLs in the destination category
are rejected. Edits require no network lookup and do not interrupt playback.
The original source title and other fetched metadata are preserved; a blank
artist falls back to the saved uploader when available. Failed saves leave the
editor open with an error and keep the library unchanged.

### Related songs

Audio Details looks up other recordings by the displayed artist using
[MusicBrainz](https://musicbrainz.org/). When metadata contains multiple
comma-separated artist names, choose which artist to look up before any
MusicBrainz request is made: press **d**, select with **Up / Down**, and press
**Enter**. A single artist is looked up automatically. Whitespace and empty
entries are ignored, and repeated names are listed only once
(case-insensitively, treating Unicode dash variants
like ordinary hyphens). This also allows names such as `Sachin-Jigar` to match
MusicBrainz's Unicode-hyphen spelling. It shows up to **five** distinct song
titles **per page for the selected artist** by default, configurable from **1** to **100** using **Related song limit**
in Settings. The current title is excluded when it matches. These are recordings by
the same artist, not personalized recommendations or a popularity ranking.
Artist names are taken from playback or saved metadata (with the uploader as
the existing fallback); missing artist metadata is shown explicitly. Artist
names are resolved to MusicBrainz IDs before fetching recordings. When several
artists have the same name, **d**, **Up / Down**, and **Enter** let you choose
the intended artist using MusicBrainz's disambiguation labels, rather than
guessing. That artist's results are reused for the current session; **f**
refreshes the lookup and lets you choose again.
Results are labeled with the selected artist. Selecting a song searches YouTube
using only that artist. Press **c** while browsing related songs to reopen the
artist picker, including while a lookup is loading or has failed. Switching
artists cancels the previous lookup, starts at page 1, and reuses cached songs
for the chosen artist. Unselected artists are not looked up.

Press **d** to focus the related-song list, **Up / Down** to select a song, and
**Enter** to search YouTube inside the app. For multiple artists, **Esc** returns
from the songs to the artist picker without leaving the section. Press **Esc**
again from the picker to return to the library. For a single artist, **Esc**
returns directly to the library. **d** also returns directly to the library.
Use **Page Down / Page Up** to move to the next / previous page for the selected
artist. The footer shows the current page and whether more songs are available.
Paging is disabled while loading or choosing an artist; selection resets on a page change.
Previously loaded pages are cached. MusicBrainz recordings are fetched in batches
of up to 100 as needed, so navigation is not restricted to the first 100 recordings.
Titles are deduplicated across batches and the current song is excluded.
Changing the artists, current song, or page size returns to page 1; **f** refreshes
from page 1 and clears only the selected artist's cached recordings.
On smaller terminals, focusing this list temporarily replaces the metadata
view so the songs remain accessible by scrolling. Playback continues in the background.
Use **+ / -** (or **=** to increase) to adjust volume by the configured step
without leaving the related-song list or artist choices.

The YouTube search uses the configured yt-dlp executable and returns up to
five videos. Select a result with **Up / Down**, then:

- Press **a** to open the existing Add dialog, choose a category, and press
  **Enter** to resolve metadata and save.
- Press **Enter** to resolve metadata, save to the source track's category,
  and play. A video already saved in that category is played without adding
  a duplicate. The playback engine must be ready.
- Press **f** to retry a failed or empty search, or **Esc** to cancel and
  return to related songs.

Lookups do not block navigation or playback. Artist changes cancel stale
MusicBrainz lookups; results are cached per artist for the current session.
MusicBrainz requests are spaced at least one second apart and include an
identifying application User-Agent. Only the selected artist name is sent to
MusicBrainz; selecting a related title sends the artist and title to YouTube
through yt-dlp. Errors and empty results are shown in the corresponding view.
Temporary MusicBrainz failures (HTTP 429, 502, 503, or 504) are retried
automatically up to twice, with increasing delays and respect for the server's
`Retry-After` header. These waits remain cancellable and do not block playback.
If the service stays unavailable, a concise error is shown instead of the full
request URL. Press **f** from the main view or related songs to refresh
MusicBrainz results, or **f** in YouTube search to retry that search.
**r** remains the repeat control in all three views.

Escape only dismisses dialogs; it does nothing in the main view. Ctrl+C does
not quit. In text-entry dialogs, `q` is ordinary text; close the dialog with
Escape before pressing `q` to quit.

Deleting a song shows a padded confirmation dialog with its title and category,
and side-by-side **Yes, delete** and **No, keep** options.
Only **y** deletes it; **n** or **Esc** keeps it. Playback is stopped only if
the confirmed song is currently playing and the playlist was saved successfully.
Save failures stay visible in the dialog so you can retry or cancel.

## Playback behavior

Automatic playback stops at the end with repeat Off, replays the current track
with repeat Track, and wraps the whole library with repeat All. Playback errors
are displayed rather than silently skipping videos; select a track and press
Space to retry. Seeking may not be available for live streams.
Recoverable mpv probe errors and cancelled extraction subprocesses are not shown
as playback failures. If playback actually fails, the error includes mpv's most
recent diagnostic.
Press **s** to stop playback without closing the engine or changing mute or
volume. **Space** starts the selected track again from the beginning.

Audio Details shows the playing track's stored thumbnail, or the selected track's
thumbnail when no track is loaded. The image sits at the top-right inside Audio
Details with one column of right padding and no separate border or label.
Images load in the background from
the saved thumbnail URL and a bounded in-memory cache reuses recent images for
the session.
This is the YouTube video thumbnail, not a separate album-cover lookup.
Thumbnail failures are shown in the image area and do not interrupt playback.
Use **Settings > Show thumbnail** to hide or display the image.
Tracks without a thumbnail retain the text-only layout. The thumbnail is hidden
when the details pane is too small, a dialog is open, or related-song browsing is
focused. When thumbnails are first enabled, ytune queries the terminal for native
image support and its character-cell pixel size. Supported Sixel, Kitty, and iTerm2 graphics render real
image pixels, with aspect-preserving high-quality resizing. Windows Terminal
1.22 and later support Sixel.
Restart ytune after updating your terminal.

Terminals without native image support use true-color Unicode half-blocks.
This fallback is inherently low resolution (two image pixels per terminal cell);
increasing the source image size cannot make it sharp.
Native-image detection or encoding errors are shown in the image area.
No external image viewer or encoder is required.

## Export

Press **e** to export every category and track, including saved metadata, to a
JSON file. The default destination is `ytune-playlists.json` in the configured
data folder. Edit the destination path or use **Ctrl+U** to clear it, then press
**Enter** to export. **Esc** cancels. Parent directories are created if needed.
Existing files are never overwritten; choose a different path if one exists.
Exports use the same JSON structure as the saved library. Exporting does not
modify the library or require a network connection.

## Import

Press **i**, type or paste the path to a JSON file produced by Export (or an
existing `library.json`), then press **Enter**. **Ctrl+U** clears the path and
**Esc** cancels. Relative paths are resolved from the working directory.

Import merges into the current library rather than replacing it:

- Categories with matching names are combined; new categories are appended.
- Duplicate URLs within the same category are skipped, including duplicates
  inside the imported file. Existing titles and metadata are kept unchanged.
  The same URL can still appear in different categories.
- New tracks retain their saved metadata. Older files without metadata are
  supported; importing does not perform network lookups or backfill metadata.
- Existing track order is preserved, with imported tracks appended to their
  categories. Empty categories in the source are ignored.
- Playback, selection, and collapsed categories are preserved.

The entire file is validated before the merged library is saved atomically.
Malformed JSON, unsupported video URLs, invalid metadata, read errors, or save
errors leave the existing library unchanged and keep the dialog open with an
error. Successful imports report the numbers of added and skipped tracks.
The source file is never modified. Import works without mpv or yt-dlp; dismiss
the missing-dependency dialog first if it is displayed.

## Settings

Press **t** to open a centered Settings overlay above the library and audio
details. Use **Tab / Shift+Tab** or
**Up / Down** to select a field, **Ctrl+U** to clear it, **Backspace** to remove
the last character, **Enter** to save, and **Esc** to return without saving.
Each setting is shown as a label on the left and a bordered value on the right.
The active control has a cyan border and a highlighted background. Text is
appended at the end of text fields. **Space** toggles a focused checkbox.

- **yt-dlp executable:** shows the resolved executable path when available.
  Enter a valid executable path, including paths containing spaces. A saved
  path takes precedence over `YTUNE_YTDLP` and PATH. Clear the field to return
  to automatic discovery (`YTUNE_YTDLP`, then PATH). Saving a changed path
  restarts the playback engine and stops current playback. Saving while the
  engine is Not Ready also retries startup. The same executable is used for
  metadata lookup and streaming.
- **mpv executable:** shows the resolved executable path when available.
  Enter the path to mpv itself (for example, `C:\tools\mpv\mpv.exe`), not a GUI
  launcher. Paths containing spaces are supported. A saved path takes precedence
  over `YTUNE_MPV` and PATH. Clear the field to return to automatic discovery
  (`YTUNE_MPV`, then PATH). Saving a changed path stops current playback and
  restarts the playback engine. Saving while the engine is Not Ready retries startup.
- **Data folder:** the folder used for `library.json`, `settings.json`, and the
  default export destination. Relative paths become absolute when saved.
- **Seek interval:** a positive whole number of seconds; default **10**.
- **Volume step:** a whole-number percentage from **1** to **100**; default **5**.
  Seek and volume changes take effect immediately without restarting playback.
- **Related song limit:** songs per artist per page, a whole number from **1** to **100**;
  default **5** per artist.
  Changes take effect immediately, including cached MusicBrainz results, without
  restarting playback. Further recording batches are fetched as needed after
  filtering distinct titles and excluding the current song. YouTube
  search still returns up to five videos.
- **Show thumbnail:** a checkbox, unchecked by default. Focus it and press
  **Space** to toggle between `[x]` (display) and `[ ]` (hide), then **Enter**
  to save. **Esc** discards the change. Saving takes effect without restarting playback and
  persists across launches. Hidden thumbnails are not downloaded, and Audio
  Details uses the full text width. Existing settings files without this field
  default to thumbnails hidden. Explicitly saved values are preserved.

Use **Tab / Shift+Tab** or **Up / Down** to navigate fields; on small terminals,
the Settings fields scroll to keep the focused field visible.

Changing the data folder copies the current library and the new settings to
the destination and switches future reads/writes there. Old files are kept.
To protect existing data, another folder already containing `library.json` or
`settings.json` is refused; choose a new folder or import its library instead.
If the switch fails, the original location remains active. A small
`location.json` locator stays in the original platform configuration directory
so the app can find the chosen folder on the next launch.

Settings are saved atomically. Invalid values or write errors leave the dialog
open with an error and preserve the active configuration. Missing settings
files in the default configuration folder use the original 10-second/5%
defaults. Invalid saved settings or missing relocated data are reported at
startup rather than silently resetting the library.

Example `settings.json`:

```json
{
  "ytdlp_path": "C:\\tools\\yt-dlp.exe",
  "mpv_path": "C:\\tools\\mpv\\mpv.exe",
  "seek_seconds": 10,
  "volume_step": 5,
  "related_song_limit": 5,
  "show_thumbnail": false
}
```

## Saved library

Changes are saved immediately and atomically to `library.json` in the
configured data folder. Before changing Settings, the default is:

- Windows: `%APPDATA%\ytune\ytune\config\library.json`
- Linux: `~/.config/ytune/library.json`
- macOS: `~/Library/Application Support/com.ytune.ytune/library.json`

Example:

```json
{
  "playlists": [
    {
      "name": "Favorites",
      "tracks": [
        {
          "title": "My saved video",
          "url": "https://www.youtube.com/watch?v=VIDEO_ID",
          "metadata": {
            "title": "Resolved video title",
            "artist": null,
            "album": null,
            "uploader": "Example channel",
            "duration": 120.0,
            "thumbnail": null,
            "upload_date": "20261018"
          }
        }
      ]
    }
  ]
}
```

Malformed or unreadable libraries cause an explicit startup error and are not
overwritten. Failed saves leave the in-memory library unchanged.

## Troubleshooting

| Problem | What to check |
| --- | --- |
| `cargo` or `rustc` is not found | Reopen the terminal after installing Rustup and check Cargo's binary directory is on PATH. |
| Build fails with a missing linker or Windows SDK | Install the Windows C++ workload and SDK, macOS command-line tools, or Linux compiler/linker packages listed above. Use current stable Rust. |
| `ytune` is not found after installation | Check Cargo's binary directory is on PATH, or run `cargo run --release --locked` from the repository. |
| Header stays **Not Ready** | Verify both playback executables, set their paths with **t**, and save Settings to retry startup. A GUI mpv launcher is not a substitute for mpv itself. |
| YouTube extraction fails or no audio format is available | Update yt-dlp, check your JavaScript runtime, network connection, and whether the video is publicly accessible. |
| Audio is silent | Check system output/mixer settings, raise ytune's volume, and press **m** if muted. |
| Only a resize message appears | Increase the terminal to at least 64 columns by 20 rows. |
| Related recordings are wrong or unavailable | Use **d** to choose the intended artist, **c** to switch credited artists, or **f** to refresh/retry. Artist metadata may need editing with **Enter** in the library. |
| Startup reports invalid JSON or missing relocated data | Back up and repair the reported files, or restore the relocated library and settings; ytune does not silently discard them. |

Executable resolution is: **saved Settings path**, then **environment override**
(`YTUNE_MPV` / `YTUNE_YTDLP`), then **PATH**. A stale saved path overrides a
working PATH entry; clear the field in Settings to return to automatic discovery.
For example, to override both executables for one PowerShell session:

```powershell
$env:YTUNE_MPV = 'C:\tools\mpv\mpv.exe'
$env:YTUNE_YTDLP = 'C:\tools\yt-dlp.exe'
ytune
```

For Linux/macOS, use `export YTUNE_MPV='/absolute/path/to/mpv'` and
`export YTUNE_YTDLP='/absolute/path/to/yt-dlp'` before launching ytune.

If yt-dlp cannot be found at its saved path, on PATH, or at `YTUNE_YTDLP`, a centered error dialog
shows setup instructions. The top-right header displays **Not Ready** in
yellow whenever the playback engine is unavailable. Press **Esc** to dismiss
the dialog, **t** to set its path in Settings, or **q** to quit; dismissing it
does not mark the engine ready.
Other errors continue to appear in the header.
Navigation, removal, import, and export still work without playback dependencies, but adding a new track
requires yt-dlp and a successful metadata lookup. Install the dependency (or
set its path in Settings or its environment override), then save Settings to
retry startup or restart the app.
Update yt-dlp when YouTube changes its extraction behavior. Restricted,
private, unavailable, or sign-in-required videos may not play. No credentials
or cookies are configured by ytune. mpv user configuration is disabled for
predictable audio-only behavior. Use only content you are authorized to access
and respect YouTube's terms.

## Development

From the repository, run:

```powershell
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
```

An opt-in integration test exercises a real mpv process with a generated silent
WAV (load, pause, seek, volume, resume, end-of-file). It requires the playback
dependencies and an audio device. Set `YTUNE_SMOKE_URL` to a YouTube video URL
to additionally verify network streaming and advancing playback:

```powershell
cargo test real_mpv_playback_smoke_test -- --ignored --nocapture
```

To verify next/previous/next during real lookups does not leave a stale
cancellation or probe error while the final track streams, set `YTUNE_SMOKE_URL`
and run:

```powershell
cargo test real_track_switch_keeps_playback_error_free -- --ignored --nocapture
```

To verify adding a real YouTube video, persisting its resolved metadata,
exporting it, and cancelling a subsequent lookup:

```powershell
$env:YTUNE_SMOKE_URL = 'https://www.youtube.com/watch?v=jNQXAC9IVRw'
cargo test real_add_resolves_and_persists_metadata -- --ignored --nocapture
```

## License

See [LICENSE](LICENSE) for the GNU General Public License, version 3.
