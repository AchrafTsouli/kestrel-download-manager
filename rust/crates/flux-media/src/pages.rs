//! Which addresses are video pages, playlists or streams, and their normal form. Same rules as 1.x
//! (`VideoPages`), and as the browser extension's `media-policy.js`.

use std::sync::LazyLock;

use regex::Regex;
use url::Url;

const YOUTUBE_HOSTS: &[&str] = &["youtube.com", "www.youtube.com", "m.youtube.com", "music.youtube.com", "www.youtube-nocookie.com", "youtube-nocookie.com", "youtu.be"];

/// Single-video page shapes on well-known sites; any other page can still be tried explicitly.
static KNOWN_PAGES: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    [
        ("vimeo.com", r"^/(?:channels/[\w-]+/|groups/[\w-]+/videos/)?\d+(?:/\w+)?/?$"),
        ("player.vimeo.com", r"^/video/\d+/?$"),
        ("dailymotion.com", r"^/video/[A-Za-z0-9]+/?$"),
        ("dai.ly", r"^/[A-Za-z0-9]+/?$"),
        ("x.com", r"^/\w+/status/\d+(?:/video/\d+)?/?$"),
        ("twitter.com", r"^/\w+/status/\d+(?:/video/\d+)?/?$"),
        ("facebook.com", r"^/(?:watch/?|reel/\d+/?|[\w.-]+/videos/(?:[\w.-]+/)?\d+/?|share/[rv]/[\w-]+/?)$"),
        ("fb.watch", r"^/[\w-]+/?$"),
        ("instagram.com", r"^/(?:[\w.]+/)?(?:p|reel|reels|tv)/[\w-]+/?$"),
        ("tiktok.com", r"^/@[\w.-]+/video/\d+/?$"),
        ("twitch.tv", r"^/(?:videos/\d+|\w+/clip/[\w-]+)/?$"),
        ("clips.twitch.tv", r"^/[\w-]+/?$"),
        ("reddit.com", r"^/r/\w+/comments/\w+(?:/[^/]*)?/?$"),
        ("streamable.com", r"^/\w+/?$"),
        ("bilibili.com", r"^/video/(?:BV\w+|av\d+)/?$"),
        ("rumble.com", r"^/v[\w-]+\.html$"),
        ("odysee.com", r"^/@[^/]+/[^/]+$"),
        ("ted.com", r"^/talks/\w+/?$"),
        // A track, not a set (the regex crate has no look-ahead; sets are excluded below).
        ("soundcloud.com", r"^/[\w-]+/[\w-]+/?$"),
    ]
    .into_iter()
    .map(|(host, path)| (host, Regex::new(path).unwrap()))
    .collect()
});
static YOUTUBE_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^/(?:shorts|embed|live)/([A-Za-z0-9_-]{11})/?$").unwrap());
static YOUTUBE_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{11}$").unwrap());
static LIST_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{2,64}$").unwrap());
static CHANNEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u)^/(@[\w.-]{1,100}|channel/UC[\w-]{22}|c/[\w.-]{1,100}|user/[\w.-]{1,100})(?:/(videos|shorts|streams|playlists))?/?$").unwrap());

fn web(value: &str) -> Option<Url> {
    if value.is_empty() || value.len() > 4096 {
        return None;
    }
    let url = Url::parse(value).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.username().is_empty() && url.password().is_none()).then_some(url)
}
fn host(url: &Url) -> String {
    url.host_str().unwrap_or_default().to_ascii_lowercase()
}
fn is_youtube_host(host: &str) -> bool {
    YOUTUBE_HOSTS.contains(&host)
}
fn host_matches(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}
fn query<'a>(url: &'a Url, name: &str) -> Option<&'a str> {
    url.query()?.split('&').map(|pair| pair.split_once('=').unwrap_or((pair, ""))).find(|(key, _)| *key == name).map(|(_, value)| value)
}

fn youtube(url: &Url) -> Option<String> {
    if url.scheme() != "https" || url.port().is_some() {
        return None;
    }
    let id = if host(url) == "youtu.be" {
        Some(url.path().trim_matches('/').to_string())
    } else if url.path() == "/watch" {
        query(url, "v").map(str::to_string)
    } else {
        YOUTUBE_PATH.captures(url.path()).map(|c| c[1].to_string())
    };
    id.filter(|id| YOUTUBE_ID.is_match(id)).map(|id| format!("https://www.youtube.com/watch?v={id}"))
}

/// Normalizes a single-video page or stream address, or returns `None` when it cannot be one.
pub fn canonicalize(value: &str) -> Option<String> {
    let mut url = web(value)?;
    if is_youtube_host(&host(&url)) {
        return youtube(&url);
    }
    url.set_fragment(None);
    Some(url.to_string())
}

pub fn is_youtube(value: &str) -> bool {
    web(value).is_some_and(|url| is_youtube_host(&host(&url)))
}

/// A YouTube playlist or channel listing, normalized; other sites' playlists are found by yt-dlp itself.
pub fn playlist(value: &str) -> Option<String> {
    let url = web(value)?;
    if url.scheme() != "https" || !is_youtube_host(&host(&url)) {
        return None;
    }
    if let Some(list) = query(&url, "list").filter(|list| LIST_ID.is_match(list)) {
        return Some(format!("https://www.youtube.com/playlist?list={list}"));
    }
    let channel = CHANNEL.captures(url.path())?;
    Some(format!("https://www.youtube.com/{}/{}", &channel[1], channel.get(2).map_or("videos", |m| m.as_str())))
}

pub fn is_stream_manifest(value: &str) -> bool {
    web(value).is_some_and(|url| {
        let path = url.path().to_ascii_lowercase();
        path.ends_with(".m3u8") || path.ends_with(".mpd")
    })
}

/// Pages that should open the video quality list rather than a plain file download.
pub fn is_known_video_page(value: &str) -> bool {
    let Some(url) = web(value) else { return false };
    let host = host(&url);
    if is_youtube_host(&host) {
        return youtube(&url).is_some() || playlist(value).is_some();
    }
    if is_stream_manifest(value) {
        return true;
    }
    let path = url.path();
    KNOWN_PAGES.iter().any(|(domain, pattern)| {
        host_matches(&host, domain)
            && pattern.is_match(path)
            && (*domain != "facebook.com" || path.trim_end_matches('/') != "/watch" || query(&url, "v").is_some())
            && (*domain != "soundcloud.com" || !path.trim_start_matches('/').split('/').nth(1).is_some_and(|s| s == "sets"))
    })
}

/// A short name for the site ("YouTube", "vimeo.com", "HLS stream").
pub fn site_name(url: &str) -> String {
    if is_stream_manifest(url) {
        return if url.to_ascii_lowercase().contains(".mpd") { "DASH stream".into() } else { "HLS stream".into() };
    }
    let host = web(url).map(|u| host(&u)).unwrap_or_default();
    if is_youtube_host(&host) {
        return "YouTube".into();
    }
    ["www.", "m.", "mobile."].iter().find_map(|p| host.strip_prefix(p)).unwrap_or(&host).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_addresses_become_one_watch_address() {
        for url in ["https://youtu.be/dQw4w9WgXcQ", "https://m.youtube.com/watch?v=dQw4w9WgXcQ&t=10", "https://www.youtube.com/shorts/dQw4w9WgXcQ", "https://www.youtube.com/embed/dQw4w9WgXcQ"] {
            assert_eq!(canonicalize(url).as_deref(), Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ"), "{url}");
        }
        assert_eq!(canonicalize("http://www.youtube.com/watch?v=dQw4w9WgXcQ"), None);
        assert_eq!(canonicalize("https://www.youtube.com/watch?v=short"), None);
        assert_eq!(canonicalize("https://user:pw@vimeo.com/1"), None);
        assert_eq!(canonicalize("https://vimeo.com/76979871#t=3").as_deref(), Some("https://vimeo.com/76979871"));
    }

    #[test]
    fn playlists_and_channels_are_normalized() {
        assert_eq!(playlist("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PLabc123").as_deref(), Some("https://www.youtube.com/playlist?list=PLabc123"));
        assert_eq!(playlist("https://www.youtube.com/@flux").as_deref(), Some("https://www.youtube.com/@flux/videos"));
        assert_eq!(playlist("https://www.youtube.com/@flux/shorts").as_deref(), Some("https://www.youtube.com/@flux/shorts"));
        assert_eq!(playlist("https://www.youtube.com/watch?v=dQw4w9WgXcQ"), None);
    }

    #[test]
    fn known_pages_open_the_quality_list() {
        for url in ["https://vimeo.com/76979871", "https://x.com/someone/status/123", "https://www.facebook.com/watch?v=1", "https://soundcloud.com/artist/track", "https://cdn.example.test/live/index.m3u8", "https://www.tiktok.com/@a.b/video/123"] {
            assert!(is_known_video_page(url), "{url}");
        }
        for url in ["https://www.facebook.com/watch", "https://soundcloud.com/artist/sets", "https://example.test/file.zip", "https://vimeo.com/about"] {
            assert!(!is_known_video_page(url), "{url}");
        }
        assert_eq!(site_name("https://www.vimeo.com/1"), "vimeo.com");
        assert_eq!(site_name("https://a.test/x.mpd"), "DASH stream");
    }
}
