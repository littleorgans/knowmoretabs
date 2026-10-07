//! A `YouTube` video as yt-dlp describes it, which captions to read, and the
//! markdown kept: what the video is, then what is said in it.
//!
//! slice: content
//! why: A video's words are in its captions, not its page. The owner's rule
//!      is English first, else the video's own language, and a person's
//!      captions before the machine's: a caption track `YouTube` translated
//!      by machine is never chosen, since the video's own language says it
//!      better. Choosing from yt-dlp's description of the video, before any
//!      caption is downloaded, means one track is fetched, never all of
//!      them. Automatic captions repeat each line as the next one rolls in,
//!      so cues are read as plain text, each line kept once, and timing is
//!      kept only as the chapter a passage belongs to. These are pure
//!      functions over yt-dlp's JSON and `WebVTT`, tested without the tool.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::content_store::{Captions, Completeness, Page};
use crate::extract;
use crate::image_pick::{Candidate, Found, Source};

/// What yt-dlp's info JSON says about a video; only what is kept or
/// chosen from is read.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Info {
    pub title: Option<String>,
    pub channel: Option<String>,
    pub uploader: Option<String>,
    /// `YYYYMMDD`.
    pub upload_date: Option<String>,
    /// Seconds.
    pub duration: Option<f64>,
    pub chapters: Option<Vec<Chapter>>,
    pub description: Option<String>,
    pub webpage_url: Option<String>,
    /// The video's own language, when `YouTube` states it.
    pub language: Option<String>,
    /// Captions a person made, by language.
    pub subtitles: BTreeMap<String, Vec<Track>>,
    /// Captions `YouTube` made, by language; the original speech's track is
    /// `<lang>-orig`, and translations carry `tlang` in their address.
    pub automatic_captions: BTreeMap<String, Vec<Track>>,
    /// The thumbnail yt-dlp prefers, its size unstated.
    pub thumbnail: Option<String>,
    pub thumbnails: Vec<Thumbnail>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Thumbnail {
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// The widest thumbnail a video's image is taken from.
const THUMBNAIL_WIDTH: u32 = 1280;

/// A video's images, best first: the thumbnail yt-dlp prefers, then the
/// listed ones no wider than [`THUMBNAIL_WIDTH`], largest first, in case
/// the preferred one does not exist.
pub fn images(info: &Info) -> Found {
    let mut sized: Vec<(u64, &str)> = info
        .thumbnails
        .iter()
        .filter_map(|t| match (t.width, t.height) {
            (Some(w), Some(h)) if w <= THUMBNAIL_WIDTH => {
                Some((u64::from(w) * u64::from(h), t.url.as_str()))
            }
            _ => None,
        })
        .collect();
    sized.sort_by_key(|(area, _)| std::cmp::Reverse(*area));
    Found::Candidates(
        info.thumbnail
            .as_deref()
            .into_iter()
            .chain(sized.into_iter().map(|(_, url)| url))
            .filter(|url| !url.is_empty())
            .map(|url| Candidate {
                url: url.to_owned(),
                source: Source::YoutubeThumbnail,
            })
            .collect(),
    )
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Track {
    pub ext: String,
    pub url: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Chapter {
    pub start_time: f64,
    pub title: String,
}

/// The caption track to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// The track's key in the info JSON, as `--sub-langs` takes it.
    pub key: String,
    pub kind: Captions,
}

impl Choice {
    /// The language, with the script and region the key has, without a
    /// track's name: yt-dlp keys a named track `<code>-<name>`, and
    /// `YouTube` writes a script `Hans` and a region `BR` or `419` in that
    /// case, so a segment in any other case is part of the name.
    pub fn lang(&self) -> String {
        let language = primary(&self.key);
        let mut subtags = self.key.split('-').skip(1).peekable();
        let script = subtags.next_if(|s| is_script(s));
        let region = subtags.next_if(|s| is_region(s));
        [Some(language.as_str()), script, region]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("-")
    }
}

/// `Hans` in `zh-Hans`.
fn is_script(subtag: &str) -> bool {
    let b = subtag.as_bytes();
    b.len() == 4 && b[0].is_ascii_uppercase() && b[1..].iter().all(u8::is_ascii_lowercase)
}

/// `BR` in `pt-BR`, `419` in `es-419`.
fn is_region(subtag: &str) -> bool {
    let b = subtag.as_bytes();
    (b.len() == 2 && b.iter().all(u8::is_ascii_uppercase))
        || (b.len() == 3 && b.iter().all(u8::is_ascii_digit))
}

const ORIGINAL: &str = "-orig";
const ENGLISH: &str = "en";
/// The one format asked for.
pub const FORMAT: &str = "vtt";

/// The track the owner's rule picks: English made by a person, then
/// English `YouTube` heard in the video, then the same two in the video's
/// own language. `None` when the video has neither.
pub fn choose(info: &Info) -> Option<Choice> {
    let mut wanted = vec![ENGLISH.to_owned()];
    if let Some(original) = original_language(info).filter(|lang| lang != ENGLISH) {
        wanted.push(original);
    }
    wanted.iter().find_map(|lang| {
        let pick = |key: Option<&String>, kind| {
            key.map(|key| Choice {
                key: key.clone(),
                kind,
            })
        };
        pick(manual(info, lang), Captions::Manual)
            .or_else(|| pick(automatic(info, lang), Captions::Automatic))
    })
}

/// The primary language of the video: what `YouTube` states, else the
/// language of its original speech track.
fn original_language(info: &Info) -> Option<String> {
    info.language
        .as_deref()
        .map(primary)
        .filter(|lang| !lang.is_empty())
        .or_else(|| {
            info.automatic_captions
                .keys()
                .find_map(|key| key.strip_suffix(ORIGINAL))
                .map(primary)
        })
}

/// `pt` from `pt-BR`, lowercased.
fn primary(lang: &str) -> String {
    lang.split(['-', '_'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// A person's track in `lang`: the plain code first, then a regional one.
fn manual<'a>(info: &'a Info, lang: &str) -> Option<&'a String> {
    let keys = usable(&info.subtitles, lang, |_| true);
    keys.iter()
        .find(|key| key.as_str() == lang)
        .or_else(|| keys.first())
        .copied()
}

/// `YouTube`'s own track of the speech in `lang`, never a translation: the
/// original track first, then the plain code.
fn automatic<'a>(info: &'a Info, lang: &str) -> Option<&'a String> {
    let keys = usable(&info.automatic_captions, lang, |tracks| {
        tracks.iter().all(|track| !track.url.contains("tlang="))
    });
    let original = format!("{lang}{ORIGINAL}");
    keys.iter()
        .find(|key| **key == &original)
        .or_else(|| keys.iter().find(|key| key.as_str() == lang))
        .or_else(|| keys.first())
        .copied()
}

/// The keys of `tracks` in language `lang` that have a `WebVTT` track, pass
/// `keep`, and are safe to pass to `--sub-langs` as they are.
fn usable<'a>(
    tracks: &'a BTreeMap<String, Vec<Track>>,
    lang: &str,
    keep: impl Fn(&[Track]) -> bool,
) -> Vec<&'a String> {
    tracks
        .iter()
        .filter(|(key, formats)| {
            let base = key.strip_suffix(ORIGINAL).unwrap_or(key);
            primary(base) == lang
                && is_plain_key(key)
                && formats.iter().any(|track| track.ext == FORMAT)
                && keep(formats)
        })
        .map(|(key, _)| key)
        .collect()
}

/// Letters, digits, `-` and `_`, not starting with `-`: `--sub-langs`
/// reads a key as a regular expression, and a leading `-` as "not".
fn is_plain_key(key: &str) -> bool {
    !key.is_empty()
        && !key.starts_with('-')
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// One caption line, from the cue that first showed it.
#[derive(Debug, Clone, PartialEq)]
pub struct Said {
    /// Seconds into the video.
    pub at: f64,
    pub text: String,
}

/// Cue text without tags, settings or timing. Automatic cues can grow or
/// rewrap the previous cue: remove only their shared suffix and prefix,
/// while cues overlap, or touch in `YouTube`'s rolling layout. Ordinary
/// touching cues and manual captions keep repeated speech.
pub fn transcript(vtt: &str, captions: Captions) -> Vec<Said> {
    let mut said = Vec::new();
    let mut previous: Vec<String> = Vec::new();
    let mut previous_end = None;
    let normalized = vtt.replace("\r\n", "\n");
    for block in normalized.split("\n\n") {
        let mut lines = block.lines();
        let Some((at, end, rolling)) = lines.find_map(|line| {
            let (start, end) = line.split_once("-->")?;
            Some((
                seconds(start.trim())?,
                seconds(end.split_whitespace().next()?)?,
                line.contains("position:0%"),
            ))
        }) else {
            continue;
        };
        let text: Vec<String> = lines.map(plain).filter(|line| !line.is_empty()).collect();
        if text.is_empty() {
            continue;
        }
        let words: Vec<&str> = text
            .iter()
            .flat_map(|line| line.split_whitespace())
            .collect();
        let before: Vec<&str> = previous
            .iter()
            .flat_map(|line| line.split_whitespace())
            .collect();
        let mut shared = if captions == Captions::Automatic
            && previous_end.is_some_and(|end| at < end || (rolling && at <= end))
        {
            (1..=before.len().min(words.len()))
                .rev()
                .find(|&n| before[before.len() - n..] == words[..n])
                .unwrap_or(0)
        } else {
            0
        };
        for line in &text {
            let rest = line
                .split_whitespace()
                .skip(shared)
                .collect::<Vec<_>>()
                .join(" ");
            shared = shared.saturating_sub(line.split_whitespace().count());
            if !rest.is_empty() {
                said.push(Said { at, text: rest });
            }
        }
        previous = text;
        previous_end = Some(end);
    }
    said
}

/// `00:01:02.500` or `01:02.500` as seconds.
fn seconds(stamp: &str) -> Option<f64> {
    stamp.split(':').try_fold(0.0, |total, part| {
        Some(total * 60.0 + part.parse::<f64>().ok()?)
    })
}

/// A cue line without its tags and entities, its whitespace collapsed.
fn plain(line: &str) -> String {
    let mut text = String::with_capacity(line.len());
    let mut in_tag = false;
    for c in line.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    let text = text
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A paragraph ends at a sentence's end once it is this long.
const PARAGRAPH: usize = 500;
/// A paragraph ends at a line's end once it is this long, sentence or not:
/// automatic captions may have no punctuation at all.
const PARAGRAPH_MOST: usize = 1200;

/// `# Title`, a list of who, when and how long, the chapters, the
/// description, then the transcript under `## Transcript`, by chapter when
/// the video has chapters. `said` is empty when there are no captions;
/// the page is thin then, keeping what the video says about itself. Ends
/// in one newline.
pub fn page(info: &Info, said: &[Said], captions: Option<Captions>) -> Page {
    let title = info
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("Untitled video")
        .to_owned();
    let mut parts = vec![format!("# {title}")];
    let mut about = Vec::new();
    if let Some(channel) = info.channel.as_deref().or(info.uploader.as_deref()) {
        about.push(format!("- Channel: {}", channel.trim()));
    }
    if let Some(date) = info.upload_date.as_deref().and_then(date) {
        about.push(format!("- Uploaded: {date}"));
    }
    if let Some(duration) = info.duration.filter(|d| *d > 0.0) {
        about.push(format!("- Duration: {}", clock(duration)));
    }
    if !about.is_empty() {
        parts.push(about.join("\n"));
    }
    let chapters = info.chapters.as_deref().unwrap_or_default();
    if !chapters.is_empty() {
        let list = chapters
            .iter()
            .map(|c| format!("- {} {}", clock(c.start_time), c.title.trim()))
            .collect::<Vec<_>>()
            .join("\n");
        parts.push(format!("## Chapters\n\n{list}"));
    }
    if let Some(description) = info.description.as_deref().map(str::trim)
        && !description.is_empty()
    {
        parts.push(format!("## Description\n\n{description}"));
    }
    if !said.is_empty() {
        parts.push("## Transcript".to_owned());
        parts.extend(sections(said, chapters));
    }
    let markdown = format!("{}\n", parts.join("\n\n"));
    Page {
        title: Some(title),
        extractor: String::new(),
        completeness: if said.is_empty() {
            Completeness::Thin
        } else {
            Completeness::Full
        },
        chars: extract::plain_chars(&markdown),
        captions,
        markdown,
    }
}

/// The transcript's paragraphs, each chapter's under `### <start> <title>`.
fn sections(said: &[Said], chapters: &[Chapter]) -> Vec<String> {
    if chapters.is_empty() {
        return paragraphs(said);
    }
    let mut out = Vec::new();
    for (i, chapter) in chapters.iter().enumerate() {
        let end = chapters.get(i + 1).map_or(f64::INFINITY, |c| c.start_time);
        // Lines before the first chapter's start belong to the first.
        let start = if i == 0 {
            f64::NEG_INFINITY
        } else {
            chapter.start_time
        };
        let inside: Vec<Said> = said
            .iter()
            .filter(|line| line.at >= start && line.at < end)
            .cloned()
            .collect();
        if inside.is_empty() {
            continue;
        }
        out.push(format!(
            "### {} {}",
            clock(chapter.start_time),
            chapter.title.trim()
        ));
        out.extend(paragraphs(&inside));
    }
    out
}

/// Lines joined into paragraphs of a readable length.
fn paragraphs(said: &[Said]) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for line in said {
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&line.text);
        let ends_sentence = current.ends_with(['.', '?', '!']);
        if (current.len() >= PARAGRAPH && ends_sentence) || current.len() >= PARAGRAPH_MOST {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// `2005-04-24` from `20050424`.
fn date(raw: &str) -> Option<String> {
    (raw.len() == 8 && raw.bytes().all(|b| b.is_ascii_digit()))
        .then(|| format!("{}-{}-{}", &raw[..4], &raw[4..6], &raw[6..]))
}

/// `1:02:03`, or `2:03` under an hour.
fn clock(seconds: f64) -> String {
    // Whole seconds, never negative; no video is near u64's limit.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let total = seconds.max(0.0) as u64;
    let (hours, minutes, secs) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    }
}

#[cfg(test)]
#[path = "youtube_page_tests.rs"]
mod tests;
