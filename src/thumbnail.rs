use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    io::Cursor,
    sync::mpsc::TryRecvError,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use image::{DynamicImage, ImageReader, RgbImage, imageops::FilterType};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    widgets::{Clear, Paragraph, Wrap},
};
use ratatui_image::{
    Resize, StatefulImage,
    picker::{Picker, ProtocolType},
    protocol::StatefulProtocol,
};

use crate::metadata::Lookup;

const MAX_DOWNLOAD_BYTES: usize = 8 * 1024 * 1024;
const CACHE_ENTRIES: usize = 8;

#[derive(Default)]
pub struct Thumbnail {
    url: Option<String>,
    pending: Option<Lookup<RgbImage>>,
    cache: VecDeque<(String, RgbImage)>,
    error: Option<String>,
    terminal_error: Option<String>,
    // The UI borrows App immutably; only the image widget's encoding state changes during drawing.
    renderer: RefCell<Option<NativeRenderer>>,
    rendered_area: Cell<Option<Rect>>,
}

impl Thumbnail {
    pub fn detect_terminal(&mut self) {
        match Picker::from_query_stdio() {
            Ok(picker) => self.set_picker(picker),
            Err(error) => {
                self.terminal_error = Some(format!("Terminal image detection failed: {error}"));
            }
        }
    }

    fn set_picker(&mut self, picker: Picker) {
        self.terminal_error = None;
        *self.renderer.get_mut() = if picker.protocol_type() == ProtocolType::Halfblocks {
            None
        } else {
            Some(NativeRenderer {
                picker,
                protocol: None,
                error: None,
            })
        };
    }

    pub fn take_rendered_area(&self) -> Option<Rect> {
        self.rendered_area.take()
    }

    pub fn rendered_area(&self) -> Option<Rect> {
        self.rendered_area.get()
    }

    pub fn update(&mut self, url: Option<&str>) {
        let url = url.map(str::trim).filter(|url| !url.is_empty());
        if self.url.as_deref() != url {
            self.pending = None;
            self.error = None;
            if let Some(renderer) = self.renderer.get_mut() {
                renderer.protocol = None;
                renderer.error = None;
            }
            self.url = url.map(str::to_owned);
            if let Some(url) = url {
                if let Some(index) = self.cache.iter().position(|(key, _)| key == url) {
                    if let Some(entry) = self.cache.remove(index) {
                        self.cache.push_back(entry);
                    }
                } else {
                    let url = url.to_owned();
                    self.pending = Some(Lookup::spawn("Thumbnail", move || async move {
                        download(&url).await
                    }));
                }
            }
        }
        let Some(pending) = &self.pending else {
            return;
        };
        let result = match pending.results.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("Thumbnail loader stopped unexpectedly".into()),
        };
        self.pending = None;
        match result {
            Ok(image) => {
                if let Some(url) = &self.url {
                    if self.cache.len() == CACHE_ENTRIES {
                        self.cache.pop_front();
                    }
                    self.cache.push_back((url.clone(), image));
                }
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub fn render(&self, frame: &mut Frame, url: &str, area: Rect) {
        if area.is_empty() {
            return;
        }
        let mut renderer = self.renderer.borrow_mut();
        if let Some(error) = &self.terminal_error {
            render_error(frame, area, error);
            return;
        }
        if self.url.as_deref() == Some(url.trim()) {
            if let Some(error) = &self.error {
                render_error(frame, area, error);
                return;
            }
            if let Some((_, image)) = self.cache.iter().find(|(key, _)| key == url.trim()) {
                if let Some(renderer) = renderer.as_mut() {
                    match renderer.render(frame, image, area) {
                        Ok(area) => self.rendered_area.set(Some(area)),
                        Err(error) => {
                            let error = format!("{error:#}");
                            renderer.error = Some(error.clone());
                            frame.render_widget(Clear, area);
                            render_error(frame, area, &error);
                        }
                    }
                } else {
                    render_image(frame, image, area);
                }
                return;
            }
        }
        frame.render_widget(Paragraph::new("Loading..."), area);
    }
}

struct NativeRenderer {
    picker: Picker,
    protocol: Option<StatefulProtocol>,
    error: Option<String>,
}

impl NativeRenderer {
    fn render(&mut self, frame: &mut Frame, image: &RgbImage, area: Rect) -> Result<Rect> {
        if let Some(error) = &self.error {
            bail!("{error}");
        }
        let protocol = self.protocol.get_or_insert_with(|| {
            self.picker
                .new_resize_protocol(DynamicImage::ImageRgb8(image.clone()))
        });
        let resize = Resize::Fit(Some(FilterType::Lanczos3));
        let size = protocol.size_for(resize.clone(), area);
        let target = Rect::new(area.right() - size.width, area.y, size.width, size.height);
        frame.render_stateful_widget(StatefulImage::new().resize(resize), target, protocol);
        if let Some(result) = protocol.last_encoding_result() {
            result.context("Cannot encode terminal thumbnail")?;
        }
        Ok(target)
    }
}

fn render_error(frame: &mut Frame, area: Rect, error: &str) {
    frame.render_widget(
        Paragraph::new(format!("Thumbnail unavailable: {error}"))
            .style(Style::default().fg(Color::LightRed))
            .wrap(Wrap { trim: true }),
        area,
    );
}

async fn download(url: &str) -> Result<RgbImage> {
    let url = reqwest::Url::parse(url).context("Invalid thumbnail URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("Thumbnail URL must use HTTP or HTTPS");
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .context("Cannot create thumbnail client")?;
    let mut response = client
        .get(url)
        .send()
        .await
        .context("Cannot download thumbnail")?
        .error_for_status()
        .context("Thumbnail server returned an error")?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_DOWNLOAD_BYTES as u64)
    {
        bail!("Thumbnail exceeds the 8 MiB download limit");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("Cannot read thumbnail data")?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_DOWNLOAD_BYTES {
            bail!("Thumbnail exceeds the 8 MiB download limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    decode(&bytes)
}

fn decode(bytes: &[u8]) -> Result<RgbImage> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("Cannot identify thumbnail image")?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().context("Cannot decode thumbnail image")?;
    let image = if image.width() > 1280 || image.height() > 1280 {
        image.resize(1280, 1280, FilterType::Lanczos3)
    } else {
        image
    };
    Ok(image.to_rgb8())
}

fn render_image(frame: &mut Frame, image: &RgbImage, area: Rect) {
    if area.is_empty() {
        return;
    }
    // Fit without stretching; each terminal cell contains two square pixels.
    let scale = (f64::from(area.width) / f64::from(image.width()))
        .min(f64::from(area.height) * 2.0 / f64::from(image.height()));
    let width = (f64::from(image.width()) * scale).floor().max(1.0) as u16;
    let pixel_height = (f64::from(image.height()) * scale).floor().max(1.0) as u16;
    let height = pixel_height.div_ceil(2);
    let x = area.right() - width;
    let y = area.y;
    let image = image::imageops::resize(
        image,
        u32::from(width),
        u32::from(pixel_height),
        FilterType::Triangle,
    );
    for row in 0..height {
        for column in 0..width {
            let top = image.get_pixel(u32::from(column), u32::from(row) * 2).0;
            let bottom_y = u32::from(row) * 2 + 1;
            let bottom = if bottom_y < image.height() {
                let [r, g, b] = image.get_pixel(u32::from(column), bottom_y).0;
                Color::Rgb(r, g, b)
            } else {
                Color::Reset
            };
            frame.buffer_mut()[(x + column, y + row)]
                .set_symbol("\u{2580}")
                .set_fg(Color::Rgb(top[0], top[1], top[2]))
                .set_bg(bottom);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, Rgb};
    use ratatui::{Terminal, backend::TestBackend};
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
    };

    pub(crate) fn loaded(url: &str) -> Thumbnail {
        Thumbnail {
            url: Some(url.into()),
            cache: VecDeque::from([(
                url.into(),
                RgbImage::from_fn(8, 2, |_, y| {
                    if y == 0 {
                        Rgb([255, 0, 0])
                    } else {
                        Rgb([0, 0, 255])
                    }
                }),
            )]),
            ..Thumbnail::default()
        }
    }

    pub(crate) fn sixel_loaded(url: &str) -> Thumbnail {
        let mut thumbnail = loaded(url);
        thumbnail.cache.front_mut().unwrap().1 = RgbImage::from_pixel(640, 360, Rgb([10, 20, 30]));
        let mut picker = Picker::from_fontsize((8, 16));
        picker.set_protocol_type(ProtocolType::Sixel);
        thumbnail.set_picker(picker);
        thumbnail
    }

    fn png() -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(RgbImage::from_pixel(640, 360, Rgb([10, 20, 30])))
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    fn server(response: Vec<u8>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/thumbnail", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
            }
            stream.write_all(&response).unwrap();
        });
        (url, handle)
    }

    #[tokio::test]
    async fn downloads_and_decodes_thumbnail() -> Result<()> {
        let bytes = png();
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        )
        .into_bytes();
        response.extend_from_slice(&bytes);
        let (url, server) = server(response);
        let image = download(&url).await?;
        server.join().unwrap();
        assert_eq!(image.dimensions(), (640, 360));
        assert_eq!(image.get_pixel(0, 0).0, [10, 20, 30]);
        Ok(())
    }

    #[tokio::test]
    async fn reports_http_errors_and_oversized_downloads() {
        for (response, expected) in [
            (
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_owned(),
                "Thumbnail server returned an error",
            ),
            (
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    MAX_DOWNLOAD_BYTES + 1
                ),
                "8 MiB download limit",
            ),
        ] {
            let (url, server) = server(response.into_bytes());
            let error = download(&url).await.unwrap_err();
            server.join().unwrap();
            assert!(error.to_string().contains(expected), "{error:#}");
        }
        assert!(
            download("file:///thumbnail.png")
                .await
                .unwrap_err()
                .to_string()
                .contains("HTTP or HTTPS")
        );
    }

    #[test]
    fn rejects_invalid_and_oversized_images() {
        assert!(decode(b"not an image").is_err());
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(RgbImage::new(4097, 1))
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        assert!(decode(bytes.get_ref()).is_err());
    }

    #[test]
    fn retains_native_resolution_without_upscaling_and_bounds_cached_images() -> Result<()> {
        for (width, height, expected) in [
            (160, 90, (160, 90)),
            (640, 360, (640, 360)),
            (2560, 1440, (1280, 720)),
        ] {
            let mut bytes = Cursor::new(Vec::new());
            DynamicImage::ImageRgb8(RgbImage::new(width, height))
                .write_to(&mut bytes, ImageFormat::Png)?;
            assert_eq!(decode(bytes.get_ref())?.dimensions(), expected);
        }
        Ok(())
    }

    #[test]
    fn renders_sixel_pixels_at_terminal_resolution_and_resizes_without_overdraw() -> Result<()> {
        let url = "https://example.test/one.png";
        let thumbnail = sixel_loaded(url);
        for (width, height) in [(40, 14), (20, 10), (10, 6), (40, 14)] {
            thumbnail.take_rendered_area();
            let mut terminal = Terminal::new(TestBackend::new(width, height))?;
            terminal.draw(|frame| thumbnail.render(frame, url, frame.area()))?;
            let area = thumbnail
                .rendered_area()
                .expect("Native image was not rendered");
            let buffer = terminal.backend().buffer();
            let data = buffer[(area.x, area.y)].symbol();
            assert!(data.starts_with("\u{1b}P"), "{data:?}");
            let raster = format!("\"1;1;{};{}", area.width * 8, area.height * 16);
            assert!(
                data.contains(&raster),
                "Missing native raster dimensions {raster}"
            );
            assert!(
                area.width * 8 > width,
                "Image still has cell-level resolution"
            );
            assert_eq!(area.y, 0, "Image is not top-aligned");
            assert_eq!(area.right(), width, "Image is not right-aligned");
            assert!(area.bottom() <= height);
            assert!(
                !buffer
                    .content
                    .iter()
                    .any(|cell| cell.symbol() == "\u{2580}"),
                "Native mode rendered half-blocks"
            );
        }
        Ok(())
    }

    #[test]
    fn native_encoder_is_cached_and_reset_when_the_displayed_track_changes() -> Result<()> {
        let url = "https://example.test/one.png";
        let mut thumbnail = sixel_loaded(url);
        let mut terminal = Terminal::new(TestBackend::new(40, 14))?;
        terminal.draw(|frame| thumbnail.render(frame, url, frame.area()))?;
        let first = terminal.backend().buffer().clone();
        terminal.draw(|frame| thumbnail.render(frame, url, frame.area()))?;
        assert_eq!(terminal.backend().buffer(), &first);
        let renderer = thumbnail.renderer.borrow();
        assert!(renderer.as_ref().unwrap().protocol.is_some());
        drop(renderer);
        thumbnail.update(None);
        let renderer = thumbnail.renderer.borrow();
        assert!(renderer.as_ref().unwrap().protocol.is_none());
        Ok(())
    }

    #[test]
    fn native_encoding_and_terminal_detection_errors_are_visible() -> Result<()> {
        let url = "https://example.test/one.png";
        let mut thumbnail = sixel_loaded(url);
        thumbnail.renderer.get_mut().as_mut().unwrap().error = Some("Encoder failed".into());
        let mut terminal = Terminal::new(TestBackend::new(50, 8))?;
        terminal.draw(|frame| thumbnail.render(frame, url, frame.area()))?;
        assert!(
            format!("{}", terminal.backend()).contains("Thumbnail unavailable: Encoder failed")
        );
        assert!(thumbnail.rendered_area().is_none());
        thumbnail.terminal_error = Some("Terminal query failed".into());
        terminal.draw(|frame| thumbnail.render(frame, url, frame.area()))?;
        assert!(format!("{}", terminal.backend()).contains("Terminal query failed"));
        Ok(())
    }

    #[test]
    fn caches_results_bounds_memory_and_clears_state_on_track_changes() {
        let mut thumbnail = Thumbnail::default();
        for index in 0..=CACHE_ENTRIES {
            let url = format!("https://example.test/{index}.png");
            thumbnail.url = Some(url.clone());
            thumbnail.pending = Some(Lookup::completed(Ok(RgbImage::new(1, 1))));
            thumbnail.update(Some(&url));
            assert!(thumbnail.pending.is_none());
            assert!(thumbnail.error.is_none());
        }
        assert_eq!(thumbnail.cache.len(), CACHE_ENTRIES);
        assert_eq!(
            thumbnail.cache.front().unwrap().0,
            "https://example.test/1.png"
        );
        thumbnail.update(Some("https://example.test/1.png"));
        assert!(thumbnail.pending.is_none());
        assert_eq!(
            thumbnail.cache.back().unwrap().0,
            "https://example.test/1.png"
        );

        thumbnail.pending = Some(Lookup::completed(Err("Offline".into())));
        thumbnail.update(Some("https://example.test/1.png"));
        assert_eq!(thumbnail.error.as_deref(), Some("Offline"));
        thumbnail.update(Some("https://example.test/2.png"));
        assert!(thumbnail.error.is_none());
        assert!(thumbnail.pending.is_none());

        // A result from the old track must not replace the new selection.
        thumbnail.pending = Some(Lookup::completed(Ok(RgbImage::new(1, 1))));
        thumbnail.update(None);
        assert!(thumbnail.url.is_none());
        assert!(thumbnail.pending.is_none());
        assert!(thumbnail.error.is_none());
        assert_eq!(thumbnail.cache.len(), CACHE_ENTRIES);
    }

    #[test]
    fn renders_colored_half_blocks_with_preserved_aspect_ratio() -> Result<()> {
        let thumbnail = loaded("https://example.test/one.png");
        let image = &thumbnail.cache.front().unwrap().1;
        let mut terminal = Terminal::new(TestBackend::new(10, 6))?;
        terminal.draw(|frame| render_image(frame, image, Rect::new(1, 1, 8, 4)))?;
        let buffer = terminal.backend().buffer();
        for x in 1..=8 {
            let cell = &buffer[(x, 1)];
            assert_eq!(cell.symbol(), "\u{2580}");
            assert_eq!(cell.fg, Color::Rgb(255, 0, 0));
            assert_eq!(cell.bg, Color::Rgb(0, 0, 255));
            assert_eq!(buffer[(x, 0)].symbol(), " ");
            assert_eq!(buffer[(x, 2)].symbol(), " ");
            assert_eq!(buffer[(x, 3)].symbol(), " ");
        }
        terminal.draw(|frame| render_image(frame, image, Rect::new(0, 0, 0, 0)))?;
        Ok(())
    }

    #[test]
    fn renders_loading_errors_and_never_a_stale_image() -> Result<()> {
        let mut thumbnail = loaded("https://example.test/one.png");
        let mut terminal = Terminal::new(TestBackend::new(40, 8))?;
        terminal
            .draw(|frame| thumbnail.render(frame, "https://example.test/two.png", frame.area()))?;
        let content = format!("{}", terminal.backend());
        assert!(content.contains("Loading..."));
        assert!(!content.contains('\u{2580}'));
        thumbnail.error = Some("Offline".into());
        terminal
            .draw(|frame| thumbnail.render(frame, "https://example.test/one.png", frame.area()))?;
        assert!(format!("{}", terminal.backend()).contains("Thumbnail unavailable: Offline"));
        Ok(())
    }
}
