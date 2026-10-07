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
mod tests {
    use serde_json::json;

    use super::*;

    fn info(value: &serde_json::Value) -> Info {
        serde_json::from_value(value.clone()).unwrap()
    }

    fn vtt() -> serde_json::Value {
        json!([{"ext": "json3", "url": "https://captions.test/a?fmt=json3"},
               {"ext": "vtt", "url": "https://captions.test/a?fmt=vtt"}])
    }

    fn translated() -> serde_json::Value {
        json!([{"ext": "vtt", "url": "https://captions.test/a?fmt=vtt&tlang=en"}])
    }

    fn chosen(value: &serde_json::Value) -> Option<(String, Captions)> {
        choose(&info(value)).map(|c| (c.key, c.kind))
    }

    #[test]
    fn english_by_a_person_comes_first() {
        let both = json!({"language": "en",
            "subtitles": {"de": vtt(), "en-GB": vtt(), "en": vtt()},
            "automatic_captions": {"en-orig": vtt(), "en": vtt()}});
        assert_eq!(chosen(&both), Some(("en".to_owned(), Captions::Manual)));
        let regional = json!({"subtitles": {"en-US": vtt(), "live_chat": vtt()}});
        assert_eq!(
            chosen(&regional),
            Some(("en-US".to_owned(), Captions::Manual))
        );
        let foreign_video = json!({"language": "de",
            "subtitles": {"de": vtt(), "en": vtt()},
            "automatic_captions": {"de-orig": vtt()}});
        assert_eq!(
            chosen(&foreign_video),
            Some(("en".to_owned(), Captions::Manual))
        );
    }

    #[test]
    fn named_tracks_record_language_without_changing_the_download_key() {
        for (key, language) in [
            ("en-captiontrack", "en"),
            ("en-CA-captiontrack", "en-CA"),
            ("en-CA", "en-CA"),
            ("en-ca-captiontrack", "en"),
            ("en-419-captiontrack", "en-419"),
            ("es-419", "es-419"),
            ("zh-Hans", "zh-Hans"),
            ("zh-Hant-captiontrack", "zh-Hant"),
            ("zh-Hant-TW", "zh-Hant-TW"),
            ("pt-BR-captiontrack", "pt-BR"),
            ("en-orig", "en"),
            ("en", "en"),
        ] {
            let described = info(&json!({
                "language": primary(key),
                "subtitles": {key: vtt()},
            }));
            let choice = choose(&described).unwrap();
            assert_eq!(choice.key, key);
            assert_eq!(choice.lang(), language);
        }
    }

    #[test]
    fn then_english_youtube_heard_in_the_video() {
        let original = json!({"language": "en",
            "automatic_captions": {"de": translated(), "en": vtt(), "en-orig": vtt()}});
        let choice = choose(&info(&original)).unwrap();
        assert_eq!(
            (choice.key.as_str(), choice.kind, choice.lang()),
            ("en-orig", Captions::Automatic, "en".to_owned())
        );
        let plain = json!({"automatic_captions": {"en": vtt()}});
        assert_eq!(chosen(&plain), Some(("en".to_owned(), Captions::Automatic)));
    }

    #[test]
    fn else_the_videos_own_language_by_a_person_then_by_youtube() {
        let manual = json!({"language": "pt-BR",
            "subtitles": {"pt-BR": vtt()},
            "automatic_captions": {"pt-orig": vtt(), "en": translated()}});
        assert_eq!(
            chosen(&manual),
            Some(("pt-BR".to_owned(), Captions::Manual))
        );
        let automatic = json!({"language": "de",
            "automatic_captions": {"de-orig": vtt(), "de": vtt(), "en": translated()}});
        assert_eq!(
            chosen(&automatic),
            Some(("de-orig".to_owned(), Captions::Automatic))
        );
        let unstated = json!({"automatic_captions": {"ja-orig": vtt(), "en": translated()}});
        assert_eq!(
            chosen(&unstated),
            Some(("ja-orig".to_owned(), Captions::Automatic)),
            "the original speech track names the language"
        );
    }

    #[test]
    fn no_usable_track_is_no_choice() {
        assert_eq!(chosen(&json!({})), None);
        let translations_only =
            json!({"language": "de", "automatic_captions": {"en": translated()}});
        assert_eq!(chosen(&translations_only), None);
        let other_language = json!({"language": "de", "subtitles": {"fr": vtt()}});
        assert_eq!(chosen(&other_language), None);
        let no_vtt =
            json!({"subtitles": {"en": [{"ext": "srv3", "url": "https://captions.test/a"}]}});
        assert_eq!(chosen(&no_vtt), None);
        let odd_key = json!({"subtitles": {"en.*": vtt(), "-en": vtt()}});
        assert_eq!(chosen(&odd_key), None, "never a pattern for --sub-langs");
    }

    #[test]
    fn rolling_automatic_captions_collapse_to_each_line_once() {
        let rolling = "WEBVTT\nKind: captions\nLanguage: en\n\n\
            00:00:00.320 --> 00:00:03.790 align:start position:0%\n \n[Music]\n\n\
            00:00:03.790 --> 00:00:03.800 align:start position:0%\n \n \n\n\
            00:00:03.800 --> 00:00:06.790 align:start position:0%\n \n\
            we're<00:00:04.039><c> no</c><00:00:04.359><c> strangers</c><00:00:04.840><c> to</c>\n\n\
            00:00:06.790 --> 00:00:06.800 align:start position:0%\nwe're no strangers to\n \n\n\
            00:00:06.800 --> 00:00:09.950 align:start position:0%\nwe're no strangers to\n\
            love.<00:00:07.800><c> You</c><00:00:08.039><c> know</c>\n\n\
            00:00:09.950 --> 00:00:09.960 align:start position:0%\nlove. You know\n \n";
        let said = |at: f64, text: &str| Said {
            at,
            text: text.to_owned(),
        };
        assert_eq!(
            transcript(rolling, Captions::Automatic),
            [
                said(0.32, "[Music]"),
                said(3.8, "we're no strangers to"),
                said(6.8, "love. You know"),
            ]
        );
    }

    #[test]
    fn a_line_said_twice_in_rolling_captions_is_kept_twice() {
        let rolling = "WEBVTT\n\n\
            00:00.000 --> 00:01.000 align:start position:0%\n \nthank<00:00.500><c> you</c>\n\n\
            00:01.000 --> 00:01.010 align:start position:0%\nthank you\n \n\n\
            00:01.010 --> 00:02.000 align:start position:0%\nthank you\nthank<00:01.500><c> you</c>\n\n\
            00:02.000 --> 00:02.010 align:start position:0%\nthank you\n \n\n\
            00:02.010 --> 00:03.000 align:start position:0%\nthank you\nso<00:02.500><c> much</c>\n";
        let said: Vec<String> = transcript(rolling, Captions::Automatic)
            .into_iter()
            .map(|said| said.text)
            .collect();
        assert_eq!(said, ["thank you", "thank you", "so much"]);
    }

    #[test]
    fn rolling_cues_keep_only_new_words_across_growth_and_rewrapping() {
        let vtt = "WEBVTT\n\n\
            00:00.000 --> 00:02.000\nWe test\n\n\
            00:01.000 --> 00:03.000\nWe test the route\n\n\
            00:02.000 --> 00:04.000\nWe test\nthe route works\n\n\
            00:03.000 --> 00:05.000\nthe route works well\n";
        assert_eq!(
            transcript(vtt, Captions::Automatic),
            [
                Said {
                    at: 0.0,
                    text: "We test".to_owned()
                },
                Said {
                    at: 1.0,
                    text: "the route".to_owned()
                },
                Said {
                    at: 2.0,
                    text: "works".to_owned()
                },
                Said {
                    at: 3.0,
                    text: "well".to_owned()
                },
            ]
        );
    }

    #[test]
    fn repeated_automatic_speech_after_a_gap_is_kept() {
        let vtt = "WEBVTT\n\n\
            00:00.000 --> 00:01.000\nAgain\n\n\
            00:02.000 --> 00:03.000\nAgain\n";
        assert_eq!(transcript(vtt, Captions::Automatic).len(), 2);
        let touching = "WEBVTT\n\n\
            00:00.000 --> 00:01.000\nPlease go\n\n\
            00:01.000 --> 00:02.000\ngo again\n";
        assert_eq!(
            transcript(touching, Captions::Automatic)[1].text,
            "go again"
        );
    }

    #[test]
    fn repeated_words_in_manual_captions_are_speech() {
        let vtt = "WEBVTT\n\n\
            00:00.000 --> 00:01.000\nGo!\nGo!\n\n\
            00:01.000 --> 00:02.000\nGo!\n";
        assert_eq!(transcript(vtt, Captions::Manual).len(), 3);
    }

    #[test]
    fn manual_captions_keep_every_line_without_timing_or_markup() {
        let manual = "WEBVTT\r\nKind: captions\r\n\r\nNOTE a comment\r\n\r\n\
            1\r\n00:01.200 --> 00:03.360 line:90%\r\n<v Speaker>First line</v>\r\n\
            second &amp; line\r\n\r\n2\r\n01:00:05.000 --> 01:00:07.000\r\n\
            <i>Third</i>&nbsp;line\r\n";
        let said = transcript(manual, Captions::Manual);
        assert_eq!(
            said,
            [
                Said {
                    at: 1.2,
                    text: "First line".to_owned()
                },
                Said {
                    at: 1.2,
                    text: "second & line".to_owned()
                },
                Said {
                    at: 3605.0,
                    text: "Third line".to_owned()
                },
            ]
        );
        assert_eq!(transcript("WEBVTT\n\n", Captions::Manual), []);
    }

    fn described() -> Info {
        info(&json!({
            "title": " A talk ", "channel": "A channel", "uploader": "someone",
            "upload_date": "20050424", "duration": 3723.4,
            "description": "What it is about.\nhttps://example.test/notes",
            "webpage_url": "https://www.youtube.com/watch?v=aBc-12_xYz9",
            "chapters": [{"start_time": 0.0, "title": "Intro"},
                         {"start_time": 60.0, "title": "Middle"},
                         {"start_time": 120.0, "title": "Empty"},
                         {"start_time": 3600.0, "title": "End"}],
        }))
    }

    #[test]
    fn a_video_page_says_what_it_is_then_what_is_said_by_chapter() {
        let said = [
            Said {
                at: 0.5,
                text: "Hello.".to_owned(),
            },
            Said {
                at: 61.0,
                text: "In the middle".to_owned(),
            },
            Said {
                at: 3700.0,
                text: "Goodbye.".to_owned(),
            },
        ];
        let page = page(&described(), &said, Some(Captions::Manual));
        assert_eq!(
            page.markdown,
            "# A talk\n\n\
             - Channel: A channel\n- Uploaded: 2005-04-24\n- Duration: 1:02:03\n\n\
             ## Chapters\n\n- 0:00 Intro\n- 1:00 Middle\n- 2:00 Empty\n- 1:00:00 End\n\n\
             ## Description\n\nWhat it is about.\nhttps://example.test/notes\n\n\
             ## Transcript\n\n\
             ### 0:00 Intro\n\nHello.\n\n\
             ### 1:00 Middle\n\nIn the middle\n\n\
             ### 1:00:00 End\n\nGoodbye.\n"
        );
        assert_eq!(page.title.as_deref(), Some("A talk"));
        assert_eq!(page.completeness, Completeness::Full);
        assert_eq!(page.captions, Some(Captions::Manual));
        assert_eq!(page.chars, extract::plain_chars(&page.markdown));
    }

    #[test]
    fn without_captions_the_page_is_thin_and_keeps_the_description() {
        let bare = info(&json!({"uploader": "someone", "description": "  "}));
        let page = page(&bare, &[], None);
        assert_eq!(page.markdown, "# Untitled video\n\n- Channel: someone\n");
        assert_eq!(page.completeness, Completeness::Thin);
        let described = super::page(&described(), &[], None);
        assert!(
            described
                .markdown
                .contains("## Description\n\nWhat it is about.")
        );
        assert!(!described.markdown.contains("## Transcript"));
    }

    #[test]
    fn a_transcript_without_chapters_is_paragraphs_of_a_readable_length() {
        let sentence = "word ".repeat(30).trim_end().to_owned() + ".";
        let said: Vec<Said> = (0..8)
            .map(|i| Said {
                at: f64::from(i),
                text: sentence.clone(),
            })
            .collect();
        let punctuated = paragraphs(&said);
        assert_eq!(punctuated.len(), 2);
        assert!(punctuated.iter().all(|p| p.len() >= PARAGRAPH));
        let unpunctuated: Vec<Said> = (0..60)
            .map(|i| Said {
                at: f64::from(i),
                text: "no stop here at all".to_owned(),
            })
            .collect();
        assert!(
            paragraphs(&unpunctuated)
                .iter()
                .all(|p| p.len() <= PARAGRAPH_MOST + 20)
        );
    }

    #[test]
    fn dates_and_clocks_read_as_people_write_them() {
        assert_eq!(date("20050424").as_deref(), Some("2005-04-24"));
        assert_eq!(date("2005-04"), None);
        assert_eq!(clock(19.0), "0:19");
        assert_eq!(clock(3723.9), "1:02:03");
        assert_eq!(seconds("00:01:02.500"), Some(62.5));
        assert_eq!(seconds("01:02.500"), Some(62.5));
        assert_eq!(seconds("soon"), None);
    }

    #[test]
    fn a_video_image_is_the_preferred_thumbnail_then_listed_ones_by_size() {
        let video = info(&json!({
            "thumbnail": "https://i.test/vi/abc/maxresdefault.webp",
            "thumbnails": [
                {"url": "https://i.test/vi/abc/default.jpg", "width": 120, "height": 90},
                {"url": "https://i.test/vi/abc/huge.jpg", "width": 1920, "height": 1080},
                {"url": "https://i.test/vi/abc/unsized.jpg"},
                {"url": "https://i.test/vi/abc/hqdefault.jpg", "width": 480, "height": 360},
            ],
        }));
        let Found::Candidates(found) = images(&video) else {
            panic!("a video names candidates");
        };
        let urls: Vec<&str> = found.iter().map(|c| c.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://i.test/vi/abc/maxresdefault.webp",
                "https://i.test/vi/abc/hqdefault.jpg",
                "https://i.test/vi/abc/default.jpg",
            ]
        );
        assert!(found.iter().all(|c| c.source == Source::YoutubeThumbnail));
        assert_eq!(images(&info(&json!({}))), Found::Candidates(Vec::new()));
    }
}
