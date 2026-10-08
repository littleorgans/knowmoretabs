//! One page's image: its candidates fetched in order through the guarded
//! fetcher, each checked before and while it is decoded, and the first
//! good one written again as a small JPEG.
//!
//! slice: content
//! why: An image address is named by the page, often on another host the
//!      browser never asked, so it is fetched by the same rules as a page:
//!      every hop guarded, cookieless, paced per host, one deadline. What
//!      comes back is untrusted bytes, so it is held to a size before it is
//!      read, must be one of the four formats the web serves by its own
//!      magic bytes, whatever its headers claim (an SVG or an error page is
//!      not), and is refused by its dimensions before a pixel is decoded,
//!      so a small file cannot claim a huge picture. What is kept is one
//!      JPEG of at most 768 pixels on its long side, upright, in sRGB, on
//!      white, with none of the original's metadata, so a photo's location
//!      never reaches the archive. A failure that may pass on the best
//!      candidate stops the attempt, so a worse image is never kept in its
//!      place.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::{self, FilterType};
use image::metadata::Orientation;
use image::{
    DynamicImage, ImageDecoder, ImageError, ImageFormat, ImageReader, Limits, RgbImage, RgbaImage,
};
use moxcms::{ColorProfile, Layout, TransformOptions};

use crate::content_fetch::{self, Passing};
use crate::fetch::{Fetcher, Refusal};
use crate::image_pick::{self, Candidate};
use crate::image_store::{Line, Status};

/// The formats asked for: those a pure Rust decoder is compiled in for.
const ACCEPT_IMAGE: &str = "image/jpeg,image/png,image/webp,image/gif;q=0.9";
/// The most of an image read: far more than any picture a page shows.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
/// Why an image over [`MAX_BYTES`] is refused; a test holds them equal.
const OVER_CAP: &str = "over 8 MiB";
/// The longest side decoded.
const MAX_SIDE: u32 = 8192;
/// The most a decoded image may take in memory.
const MAX_ALLOC: u64 = 128 * 1024 * 1024;
/// The long side of a kept image; a smaller one is never enlarged.
pub const LONG_EDGE: u32 = 768;
const QUALITY: u8 = 80;
/// Candidates fetched for one page at most.
pub const FETCHES: usize = 3;

/// An image kept: the JPEG and its sizes before and after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    pub jpeg: Vec<u8>,
    pub source: (u32, u32),
    pub size: (u32, u32),
}

/// A page's image attempt: its line, without the page's address or the
/// run's attempt number yet, and the JPEG when one was kept.
#[derive(Debug, Clone)]
pub struct Captured {
    pub line: Line,
    pub jpeg: Option<Vec<u8>>,
}

impl Captured {
    /// An attempt that kept nothing, `status` for `reason`, having tried
    /// `candidates`.
    pub fn ended(status: Status, reason: impl Into<String>, candidates: &[Candidate]) -> Self {
        let mut line = Line::new("", status).with_reason(reason);
        line.candidates = candidates.iter().take(image_pick::KEPT).cloned().collect();
        Self { line, jpeg: None }
    }
}

/// How one candidate ended when it was not kept.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Failure {
    /// Not an image to keep: the next candidate is tried.
    Rejected(String),
    /// A failure that may pass: the attempt ends as `error`, its HTTP
    /// status when there was one.
    Transient(String, Option<u16>),
}

/// One candidate fetched and kept, and what its line records of it.
struct Fetched {
    kept: Kept,
    final_url: String,
    http_status: u16,
    mime: String,
}

/// Tries `candidates` in order, at most [`FETCHES`] of them, and keeps the
/// first good image. A passing failure is retried as a page is; a failure
/// that may pass ends the attempt as `error`, since what follows is a
/// worse image. Never fails: a failure is an outcome too.
pub fn capture(fetcher: &Fetcher, candidates: &[Candidate]) -> Captured {
    if candidates.is_empty() {
        return Captured::ended(Status::None, "no_candidate", candidates);
    }
    let mut first_rejection = None;
    for candidate in candidates.iter().take(FETCHES) {
        match content_fetch::retrying(fetcher, || once(fetcher, &candidate.url)) {
            Ok(image) => return kept(candidate, image),
            Err(Failure::Rejected(why)) => {
                first_rejection.get_or_insert(why);
            }
            Err(Failure::Transient(why, http)) => {
                let mut ended = Captured::ended(Status::Error, why, candidates);
                ended.line.http_status = http;
                ended.line.image_url = Some(candidate.url.clone());
                return ended;
            }
        }
    }
    let why = first_rejection.unwrap_or_default();
    Captured::ended(Status::None, format!("all_rejected: {why}"), candidates)
}

fn kept(candidate: &Candidate, fetched: Fetched) -> Captured {
    let mut line = Line::new("", Status::Ok);
    line.source = Some(candidate.source);
    line.image_url = Some(candidate.url.clone());
    line.final_url = Some(fetched.final_url);
    line.http_status = Some(fetched.http_status);
    line.mime = Some(fetched.mime).filter(|mime| !mime.is_empty());
    line.source_width = Some(fetched.kept.source.0);
    line.source_height = Some(fetched.kept.source.1);
    line.width = Some(fetched.kept.size.0);
    line.height = Some(fetched.kept.size.1);
    line.generated = Some(candidate.generated());
    Captured {
        line,
        jpeg: Some(fetched.kept.jpeg),
    }
}

type Once = Result<Result<Fetched, Failure>, Passing<Result<Fetched, Failure>>>;

/// One GET of image `raw`, and what came of it. Connection trouble that
/// may pass, a 429 and a 5xx are transient, as they are for a page; a name
/// that does not resolve, a refused connection or a rule is this
/// candidate's end.
fn once(fetcher: &Fetcher, raw: &str) -> Once {
    let transient = |why: String, http: Option<u16>| Err(Failure::Transient(why, http));
    let mut response = match fetcher.get(raw, ACCEPT_IMAGE) {
        Ok(response) => response,
        Err(Refusal::Failed(why)) if content_fetch::is_passing(&why) => {
            return Err(Passing::after(transient(why, None), None));
        }
        Err(refusal) => return Ok(Err(Failure::Rejected(refusal.reason()))),
    };
    let status = response.status;
    if status == 429 {
        fetcher.slow_down(&response.url);
    }
    if !(200..300).contains(&status) {
        let why = format!("HTTP {status}");
        return match answered(status) {
            Answered::Retry => {
                let wait = content_fetch::asked_wait(&response);
                Err(Passing::after(transient(why, Some(status)), wait))
            }
            Answered::Error => Ok(transient(why, Some(status))),
            Answered::Rejected => Ok(Err(Failure::Rejected(why))),
        };
    }
    let mime = response.mime();
    let length = response
        .header("content-length")
        .and_then(|value| value.trim().parse().ok());
    if let Err(why) = admit(&mime, length, response.is_identity()) {
        return Ok(Err(Failure::Rejected(why)));
    }
    let bytes = match response.read(MAX_BYTES + 1, false) {
        Ok(bytes) => bytes,
        Err(why) if content_fetch::is_passing(&why) => {
            return Err(Passing::after(transient(why, Some(status)), None));
        }
        Err(why) => return Ok(Err(Failure::Rejected(why))),
    };
    Ok(process(&bytes)
        .map(|kept| Fetched {
            kept,
            final_url: response.url.to_string(),
            http_status: status,
            mime,
        })
        .map_err(|why| Failure::Rejected(why.to_owned())))
}

/// What an HTTP status other than success means for an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answered {
    /// Not there, or not for us: the next candidate is tried.
    Rejected,
    /// A server failure: the attempt ends as `error`.
    Error,
    /// A site asking us to come back: retried in the run, as a page is.
    Retry,
}

fn answered(status: u16) -> Answered {
    match content_fetch::status_outcome(status) {
        Some((_, true)) => Answered::Retry,
        _ if status >= 500 => Answered::Error,
        _ => Answered::Rejected,
    }
}

/// Whether a response's headers allow reading its body as an image: a
/// type that can be one, a declared length within [`MAX_BYTES`], and no
/// content encoding.
fn admit(mime: &str, length: Option<u64>, identity: bool) -> Result<(), String> {
    let image_type =
        mime.is_empty() || mime.starts_with("image/") || mime == "application/octet-stream";
    if !image_type {
        return Err(format!("not an image ({mime})"));
    }
    if length.is_some_and(|length| length > MAX_BYTES as u64) {
        return Err(OVER_CAP.to_owned());
    }
    if !identity {
        return Err("unsupported content encoding".to_owned());
    }
    Ok(())
}

/// Turns the bytes of an image into the JPEG kept of it, or says why not.
/// The format is read from the bytes, never from what the server said.
pub fn process(bytes: &[u8]) -> Result<Kept, &'static str> {
    if bytes.len() > MAX_BYTES {
        return Err(OVER_CAP);
    }
    let format = image::guess_format(bytes)
        .ok()
        .filter(|format| {
            matches!(
                format,
                ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP | ImageFormat::Gif
            )
        })
        .ok_or("not a JPEG, PNG, WebP or GIF")?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|err| unreadable(&err))?;
    let (width, height) = decoder.dimensions();
    if let Some(why) = image_pick::misfit(width, height) {
        return Err(why);
    }
    if decoder.total_bytes() > MAX_ALLOC {
        return Err("too large to decode");
    }
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let profile = decoder.icc_profile().ok().flatten();
    // The first frame of an animation is the one decoded.
    let mut image = DynamicImage::from_decoder(decoder).map_err(|err| unreadable(&err))?;
    image.apply_orientation(orientation);
    let mut rgba = image.into_rgba8();
    if let Some(profile) = profile {
        to_srgb(&mut rgba, &profile);
    }
    let source = rgba.dimensions();
    let size = shrunk(source);
    let flat = on_white(&rgba);
    let flat = if size == source {
        flat
    } else {
        imageops::resize(&flat, size.0, size.1, FilterType::Lanczos3)
    };
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, QUALITY)
        .encode_image(&flat)
        .map_err(|_| "could not be written as JPEG")?;
    Ok(Kept { jpeg, source, size })
}

fn unreadable(err: &ImageError) -> &'static str {
    match err {
        ImageError::Limits(_) => "too large to decode",
        ImageError::Unsupported(_) => "unsupported image",
        _ => "unreadable image",
    }
}

/// `source` scaled so its long side is at most [`LONG_EDGE`], never up.
fn shrunk((width, height): (u32, u32)) -> (u32, u32) {
    let long = width.max(height);
    if long <= LONG_EDGE {
        return (width, height);
    }
    let scale = |side: u32| {
        let scaled =
            (u64::from(side) * u64::from(LONG_EDGE) + u64::from(long) / 2) / u64::from(long);
        u32::try_from(scaled).unwrap_or(LONG_EDGE).max(1)
    };
    (scale(width), scale(height))
}

/// The image in sRGB, by the colour profile it carries. A profile that
/// cannot be read or applied leaves it as it is, taken to be sRGB already,
/// as most of the web is.
fn to_srgb(rgba: &mut RgbaImage, profile: &[u8]) {
    let Ok(source) = ColorProfile::new_from_slice(profile) else {
        return;
    };
    let Ok(transform) = source.create_transform_8bit(
        Layout::Rgba,
        &ColorProfile::new_srgb(),
        Layout::Rgba,
        TransformOptions::default(),
    ) else {
        return;
    };
    let mut converted = vec![0; rgba.as_raw().len()];
    if transform.transform(rgba.as_raw(), &mut converted).is_ok()
        && let Some(image) = RgbaImage::from_raw(rgba.width(), rgba.height(), converted)
    {
        *rgba = image;
    }
}

/// The image laid on white, so transparency reads as a page would show it.
fn on_white(rgba: &RgbaImage) -> RgbImage {
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let alpha = u16::from(a);
        let blend = |c: u8| {
            let mixed = (u16::from(c) * alpha + 255 * (255 - alpha) + 127) / 255;
            u8::try_from(mixed).unwrap_or(u8::MAX)
        };
        image::Rgb([blend(r), blend(g), blend(b)])
    })
}

#[cfg(test)]
mod tests {
    use image::{ImageEncoder, Rgba};

    use super::*;

    fn picture(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            Rgba([
                u8::try_from(x % 256).unwrap(),
                u8::try_from(y % 256).unwrap(),
                128,
                255,
            ])
        })
    }

    fn encoded(image: &RgbaImage, format: ImageFormat) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        let image = DynamicImage::ImageRgba8(image.clone());
        let image = if format == ImageFormat::Jpeg {
            DynamicImage::ImageRgb8(image.into_rgb8())
        } else {
            image
        };
        image.write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }

    fn decoded(jpeg: &[u8]) -> RgbImage {
        image::load_from_memory_with_format(jpeg, ImageFormat::Jpeg)
            .unwrap()
            .into_rgb8()
    }

    #[test]
    fn each_format_is_kept_as_a_jpeg_no_longer_than_768_and_never_enlarged() {
        let kept = process(&encoded(&picture(1200, 630), ImageFormat::Png)).unwrap();
        assert_eq!((kept.source, kept.size), ((1200, 630), (768, 403)));
        assert_eq!(decoded(&kept.jpeg).dimensions(), (768, 403));
        for format in [ImageFormat::Jpeg, ImageFormat::WebP, ImageFormat::Gif] {
            let kept = process(&encoded(&picture(320, 240), format)).unwrap();
            assert_eq!(
                (kept.source, kept.size),
                ((320, 240), (320, 240)),
                "{format:?}"
            );
            assert_eq!(image::guess_format(&kept.jpeg).unwrap(), ImageFormat::Jpeg);
        }
        let tall = process(&encoded(&picture(400, 1000), ImageFormat::Png)).unwrap();
        assert_eq!(tall.size, (307, 768));
    }

    #[test]
    fn small_and_strip_images_are_refused() {
        assert_eq!(
            process(&encoded(&picture(150, 300), ImageFormat::Png)),
            Err("too small")
        );
        assert_eq!(
            process(&encoded(&picture(1000, 250), ImageFormat::Png)),
            Err("out of proportion")
        );
    }

    #[test]
    fn the_bytes_decide_the_format_and_svg_or_html_is_refused() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="900" height="600"></svg>"#;
        assert_eq!(process(svg), Err("not a JPEG, PNG, WebP or GIF"));
        assert_eq!(
            process(b"<!doctype html><title>Not found</title>"),
            Err("not a JPEG, PNG, WebP or GIF")
        );
        assert_eq!(process(b""), Err("not a JPEG, PNG, WebP or GIF"));
        let truncated = encoded(&picture(400, 400), ImageFormat::Png);
        assert_eq!(process(&truncated[..60]), Err("unreadable image"));
    }

    #[test]
    fn http_failures_reject_the_candidate_unless_they_may_pass() {
        for status in [400, 401, 403, 404, 410, 451] {
            assert_eq!(answered(status), Answered::Rejected, "{status}");
        }
        for status in [500, 501, 505] {
            assert_eq!(answered(status), Answered::Error, "{status}");
        }
        for status in [429, 502, 503, 504] {
            assert_eq!(answered(status), Answered::Retry, "{status}");
        }
    }

    #[test]
    fn headers_must_allow_an_image_within_the_cap() {
        assert_eq!(admit("image/png", Some(1000), true), Ok(()));
        assert_eq!(admit("", None, true), Ok(()));
        assert_eq!(admit("application/octet-stream", None, true), Ok(()));
        assert_eq!(
            admit("text/html", Some(1000), true),
            Err("not an image (text/html)".to_owned())
        );
        assert_eq!(
            admit("image/jpeg", Some(MAX_BYTES as u64 + 1), true),
            Err("over 8 MiB".to_owned())
        );
        assert_eq!(
            admit("image/jpeg", None, false),
            Err("unsupported content encoding".to_owned())
        );
        let mut oversize = encoded(&picture(400, 400), ImageFormat::Png);
        oversize.resize(MAX_BYTES + 1, 0);
        assert_eq!(process(&oversize), Err(OVER_CAP));
        assert_eq!(OVER_CAP, format!("over {} MiB", MAX_BYTES >> 20));
    }

    #[test]
    fn a_picture_claiming_huge_dimensions_is_refused_before_it_is_decoded() {
        let mut bomb = encoded(&picture(16, 16), ImageFormat::Jpeg);
        // The baseline frame header: FF C0, length, precision, then height
        // and width as big-endian u16.
        let sof = bomb.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        bomb[sof + 5..sof + 9].copy_from_slice(&[0xEA, 0x60, 0xEA, 0x60]);
        assert_eq!(process(&bomb), Err("too large to decode"));
        let mut within_sides = encoded(&picture(16, 16), ImageFormat::Jpeg);
        within_sides[sof + 5..sof + 9].copy_from_slice(&[0x1F, 0x40, 0x1F, 0x40]);
        assert_eq!(
            process(&within_sides),
            Err("too large to decode"),
            "8000 square fits the side limit but not the memory one"
        );
    }

    #[test]
    fn exif_orientation_is_applied_and_metadata_is_not_kept() {
        let mut bytes = Cursor::new(Vec::new());
        let mut encoder = JpegEncoder::new_with_quality(&mut bytes, 90);
        // A minimal EXIF block: little-endian TIFF with one Orientation
        // entry, 6 (rotate 90 clockwise).
        let exif = [
            b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0,
            0,
        ];
        encoder.set_exif_metadata(exif.to_vec()).unwrap();
        let wide = DynamicImage::ImageRgba8(picture(600, 300)).into_rgb8();
        encoder
            .write_image(wide.as_raw(), 600, 300, image::ExtendedColorType::Rgb8)
            .unwrap();
        let kept = process(&bytes.into_inner()).unwrap();
        assert_eq!(kept.source, (300, 600), "rotated upright");
        let mut decoder = ImageReader::new(Cursor::new(&kept.jpeg))
            .with_guessed_format()
            .unwrap()
            .into_decoder()
            .unwrap();
        assert_eq!(decoder.exif_metadata().unwrap(), None);
        assert_eq!(decoder.icc_profile().unwrap(), None);
    }

    #[test]
    fn transparency_is_laid_on_white() {
        let clear = RgbaImage::from_pixel(300, 300, Rgba([0, 0, 0, 0]));
        let kept = process(&encoded(&clear, ImageFormat::Png)).unwrap();
        let pixel = decoded(&kept.jpeg).get_pixel(150, 150).0;
        assert!(pixel.iter().all(|c| *c > 250), "{pixel:?}");
    }

    #[test]
    fn a_colour_profile_is_brought_into_srgb() {
        let red = RgbaImage::from_pixel(300, 300, Rgba([200, 30, 30, 255]));
        let plain = process(&encoded(&red, ImageFormat::Png)).unwrap();
        let mut bytes = Cursor::new(Vec::new());
        let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
        encoder
            .set_icc_profile(ColorProfile::new_display_p3().encode().unwrap())
            .unwrap();
        encoder
            .write_image(red.as_raw(), 300, 300, image::ExtendedColorType::Rgba8)
            .unwrap();
        let profiled = process(&bytes.into_inner()).unwrap();
        let at = |kept: &Kept| decoded(&kept.jpeg).get_pixel(150, 150).0;
        assert!(
            at(&profiled)[0] > at(&plain)[0] + 10,
            "a Display P3 red is redder in sRGB: {:?} against {:?}",
            at(&profiled),
            at(&plain)
        );
    }

    #[test]
    fn sizes_shrink_to_the_long_edge_and_never_grow() {
        assert_eq!(shrunk((4096, 2160)), (768, 405));
        assert_eq!(shrunk((768, 100)), (768, 100));
        assert_eq!(shrunk((500, 300)), (500, 300));
        assert_eq!(shrunk((1000, 8192)), (94, 768));
    }

    #[test]
    fn nothing_to_try_is_none_and_kept_candidates_are_capped() {
        let none = Captured::ended(Status::None, "no_candidate", &[]);
        assert_eq!(none.line.status, Status::None);
        assert_eq!(none.line.candidates, []);
        let many: Vec<Candidate> = (0..8)
            .map(|i| Candidate {
                url: format!("https://a.test/{i}.jpg"),
                source: image_pick::Source::BodyImg,
            })
            .collect();
        let ended = Captured::ended(Status::Error, "timeout", &many);
        assert_eq!(ended.line.candidates.len(), image_pick::KEPT);
    }
}
