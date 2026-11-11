//! Download pages: a link that answers with a web page ("your download will start in 5 seconds…")
//! instead of the file. The browser follows the page's refresh or its link; so does the engine.

use url::Url;

/// `&amp;` and the few entities found in addresses.
fn unescape(text: &str) -> String {
    text.replace("&amp;", "&").replace("&#38;", "&").replace("&quot;", "\"").replace("&#39;", "'").replace("&#x2F;", "/").replace("&#47;", "/")
}

/// An http(s) address that is not `base` itself (a page that only reloads itself leads nowhere).
fn onward(target: &str, base: &Url) -> Option<Url> {
    let next = base.join(unescape(target.trim()).trim()).ok()?;
    let same = next.as_str().split('#').next() == base.as_str().split('#').next();
    (matches!(next.scheme(), "http" | "https") && !same).then_some(next)
}

/// A `Refresh` value: `5; url=https://…`, `0;URL='…'`, `3, https://…`.
pub fn refresh(value: &str, base: &Url) -> Option<Url> {
    let (delay, rest) = value.split_once([';', ','])?;
    delay.trim().parse::<f64>().ok()?;
    let rest = rest.trim();
    let target = if rest.len() > 3 && rest[..3].eq_ignore_ascii_case("url") && rest[3..].trim_start().starts_with('=') { rest[3..].trim_start()[1..].trim() } else { rest };
    onward(target.trim_matches(['"', '\'']), base)
}

/// The value of `name=` in one tag, quoted or not.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find(name) {
        let at = from + found;
        from = at + name.len();
        // A whole attribute name: preceded by a space, followed by `=`.
        if !lower[..at].ends_with(|c: char| c.is_whitespace()) {
            continue;
        }
        let rest = tag[at + name.len()..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let rest = rest.trim_start();
        return Some(match rest.chars().next() {
            Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or("").to_string(),
            _ => rest.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or("").to_string(),
        });
    }
    None
}

/// Every `<name …>` tag of the page.
fn tags<'a>(page: &'a str, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    let lower = page.to_ascii_lowercase();
    let open = format!("<{name}");
    let mut starts = Vec::new();
    let mut from = 0;
    while let Some(found) = lower[from..].find(&open) {
        let at = from + found;
        from = at + open.len();
        if lower[from..].starts_with(|c: char| c.is_whitespace() || c == '/' || c == '>')
            && let Some(end) = lower[at..].find('>')
        {
            starts.push((at, at + end + 1));
        }
    }
    starts.into_iter().map(move |(a, b)| &page[a..b])
}

/// Where `<meta http-equiv="refresh" content="…">` leads.
pub fn meta_refresh(page: &str, base: &Url) -> Option<Url> {
    tags(page, "meta").find_map(|tag| {
        attribute(tag, "http-equiv").filter(|v| v.trim().eq_ignore_ascii_case("refresh"))?;
        refresh(&attribute(tag, "content")?, base)
    })
}

/// The file's name at the end of an address, decoded and in lower case.
fn last_name(url: &Url) -> String {
    let segment = url.path_segments().and_then(|mut s| s.next_back()).unwrap_or("");
    let bytes: Vec<u8> = {
        let raw = segment.as_bytes();
        let mut out = Vec::with_capacity(raw.len());
        let mut i = 0;
        while i < raw.len() {
            if raw[i] == b'%' && i + 2 < raw.len() {
                if let Ok(v) = u8::from_str_radix(&segment[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
            out.push(raw[i]);
            i += 1;
        }
        out
    };
    String::from_utf8_lossy(&bytes).to_lowercase()
}

/// A link on the page to a file with the same name as the one asked for (the page's "click here
/// if the download does not start"). Only names that look like files count.
pub fn same_name_link(page: &str, base: &Url, wanted: &Url) -> Option<Url> {
    let name = last_name(wanted);
    if name.is_empty() || !name.contains('.') || name.ends_with(".html") || name.ends_with(".htm") || name.ends_with(".php") {
        return None;
    }
    tags(page, "a").filter_map(|tag| attribute(tag, "href")).filter_map(|href| onward(&href, base)).find(|link| last_name(link) == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://get.example.org/vlc/3.0.24/win64/vlc-3.0.24-win64.exe").unwrap()
    }

    #[test]
    fn follows_a_meta_refresh() {
        let page = r#"<html><head><META HTTP-EQUIV="refresh" content="5;URL='https://mirror.example.net/videolan/vlc-3.0.24-win64.exe?a=1&amp;b=2'" /></head></html>"#;
        assert_eq!(meta_refresh(page, &base()).unwrap().as_str(), "https://mirror.example.net/videolan/vlc-3.0.24-win64.exe?a=1&b=2");
    }

    #[test]
    fn reads_refresh_values() {
        assert_eq!(refresh("0; url=next.bin", &base()).unwrap().as_str(), "https://get.example.org/vlc/3.0.24/win64/next.bin");
        assert_eq!(refresh("3, https://x.example/a.zip", &base()).unwrap().as_str(), "https://x.example/a.zip");
        assert!(refresh("10", &base()).is_none(), "a reload without an address leads nowhere");
        assert!(refresh("1;url=vlc-3.0.24-win64.exe", &base()).is_none(), "the same page");
        assert!(refresh("0;url=javascript:alert(1)", &base()).is_none());
    }

    #[test]
    fn finds_the_link_to_the_same_file() {
        let page = r#"<a href="/">Home</a><a class=dl href=https://ftp.example.pt/pub/vlc-3.0.24-win64.exe>click here</a>"#;
        assert_eq!(same_name_link(page, &base(), &base()).unwrap().as_str(), "https://ftp.example.pt/pub/vlc-3.0.24-win64.exe");
        assert!(same_name_link(r#"<a href="https://x.example/other.exe">x</a>"#, &base(), &base()).is_none());
    }
}
