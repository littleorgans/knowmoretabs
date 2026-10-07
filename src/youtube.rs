//! What a `YouTube` address is: one video, a listing of videos, or some
//! other page.
//!
//! slice: content
//! why: A video is opened at many addresses (`/watch?v=`, `/shorts/`,
//!      `youtu.be`, on the `www`, `m` and `music` hosts, with a start time
//!      or a playlist attached), and each should be one fetch of the one
//!      video its id names. Channels and playlists list videos rather than
//!      being one, so they are not documents. Reading this from the address
//!      alone lets the plan settle every request before it sends any, and
//!      the request yt-dlp makes is rebuilt from the id, so nothing else of
//!      the library address leaves the machine.

use url::Url;

/// What a `YouTube` address shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Address {
    /// One video, by its id.
    Video(String),
    /// A channel or a playlist: it lists videos.
    Listing,
    /// Any other `YouTube` page, such as the home page.
    Other,
}

impl Address {
    /// What `url` shows; `None` when it is not on `YouTube`. A watch address's
    /// `list=` and `t=` are ignored: it is the one video either way.
    pub fn of(url: &Url) -> Option<Self> {
        let host = url.host_str()?.to_ascii_lowercase();
        let mut segments = url
            .path_segments()
            .into_iter()
            .flatten()
            .filter(|segment| !segment.is_empty());
        let first = segments.next();
        if host == "youtu.be" {
            return Some(first.and_then(video_id).map_or(Self::Other, Self::Video));
        }
        let site = host
            .strip_prefix("www.")
            .or_else(|| host.strip_prefix("m."))
            .or_else(|| host.strip_prefix("music."))
            .unwrap_or(&host);
        if site != "youtube.com" {
            return None;
        }
        let video = match first {
            Some("watch") => url
                .query_pairs()
                .find(|(key, _)| key == "v")
                .and_then(|(_, id)| video_id(&id)),
            Some("shorts") => segments.next().and_then(video_id),
            Some(first) if first.starts_with('@') => return Some(Self::Listing),
            Some("channel" | "c" | "user" | "playlist") => return Some(Self::Listing),
            _ => None,
        };
        Some(video.map_or(Self::Other, Self::Video))
    }
}

/// The address yt-dlp is given for video `id`: the plain watch page,
/// whatever address the library holds.
pub fn watch_url(id: &str) -> String {
    format!("https://www.youtube.com/watch?v={id}")
}

/// `id` when it has the shape of a video id: eleven letters, digits, `-`
/// or `_`.
fn video_id(id: &str) -> Option<String> {
    (id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
    .then(|| id.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(raw: &str) -> Option<Address> {
        Address::of(&Url::parse(raw).unwrap())
    }

    fn video(id: &str) -> Address {
        Address::Video(id.to_owned())
    }

    #[test]
    fn every_video_address_names_its_id() {
        for raw in [
            "https://www.youtube.com/watch?v=aBc-12_xYz9",
            "https://youtube.com/watch?v=aBc-12_xYz9",
            "https://m.youtube.com/watch?v=aBc-12_xYz9",
            "https://music.youtube.com/watch?v=aBc-12_xYz9",
            "https://WWW.YouTube.com/watch?v=aBc-12_xYz9",
            "https://www.youtube.com/watch?feature=share&v=aBc-12_xYz9",
            "https://www.youtube.com/watch?v=aBc-12_xYz9&t=95s",
            "https://www.youtube.com/watch?v=aBc-12_xYz9#t=1m35s",
            "https://www.youtube.com/watch?v=aBc-12_xYz9&list=PL0000&index=3",
            "https://www.youtube.com/watch?v=aBc-12_xYz9&start_radio=1",
            "https://www.youtube.com/shorts/aBc-12_xYz9",
            "https://www.youtube.com/shorts/aBc-12_xYz9?feature=share",
            "https://m.youtube.com/shorts/aBc-12_xYz9/",
            "https://youtu.be/aBc-12_xYz9",
            "https://youtu.be/aBc-12_xYz9?t=30&si=abcdef",
        ] {
            assert_eq!(address(raw), Some(video("aBc-12_xYz9")), "{raw}");
        }
    }

    #[test]
    fn malformed_ids_are_not_videos() {
        for raw in [
            "https://www.youtube.com/watch?v=short",
            "https://www.youtube.com/watch?v=aBc-12_xYz9X",
            "https://www.youtube.com/watch?v=aBc%2012xYz9",
            "https://www.youtube.com/watch?v=",
            "https://www.youtube.com/watch",
            "https://www.youtube.com/watch?vid=aBc-12_xYz9",
            "https://www.youtube.com/shorts/",
            "https://www.youtube.com/shorts/aBc.12_xYz9",
            "https://youtu.be/",
            "https://youtu.be/aBc-12_xYz",
            "https://www.youtube.com/",
            "https://www.youtube.com/feed/subscriptions",
        ] {
            assert_eq!(address(raw), Some(Address::Other), "{raw}");
        }
    }

    #[test]
    fn channels_and_playlists_are_listings() {
        for raw in [
            "https://www.youtube.com/@someone",
            "https://www.youtube.com/@someone/videos",
            "https://www.youtube.com/channel/UC0000",
            "https://www.youtube.com/c/Someone",
            "https://www.youtube.com/user/someone",
            "https://www.youtube.com/playlist?list=PL0000",
            "https://music.youtube.com/playlist?list=PL0000",
            "https://m.youtube.com/@someone",
        ] {
            assert_eq!(address(raw), Some(Address::Listing), "{raw}");
        }
    }

    #[test]
    fn other_hosts_are_not_youtube() {
        for raw in [
            "https://example.test/watch?v=aBc-12_xYz9",
            "https://notyoutube.com/watch?v=aBc-12_xYz9",
            "https://youtube.com.example.test/watch?v=aBc-12_xYz9",
            "https://studio.youtube.com/video/aBc-12_xYz9",
            "https://youtu.be.example.test/aBc-12_xYz9",
        ] {
            assert_eq!(address(raw), None, "{raw}");
        }
    }

    #[test]
    fn yt_dlp_gets_the_plain_watch_address() {
        assert_eq!(
            watch_url("aBc-12_xYz9"),
            "https://www.youtube.com/watch?v=aBc-12_xYz9"
        );
    }
}
