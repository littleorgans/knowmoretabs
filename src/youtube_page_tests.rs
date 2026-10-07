//! Regression checks for `youtube_page`.
//!
//! slice: content
//! why: Synthetic video descriptions and caption tracks verify selection and markdown independently of external tools and network requests.

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
    let translations_only = json!({"language": "de", "automatic_captions": {"en": translated()}});
    assert_eq!(chosen(&translations_only), None);
    let other_language = json!({"language": "de", "subtitles": {"fr": vtt()}});
    assert_eq!(chosen(&other_language), None);
    let no_vtt = json!({"subtitles": {"en": [{"ext": "srv3", "url": "https://captions.test/a"}]}});
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
