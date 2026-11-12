//! The site grabber: follows a site's pages (same address, at most 20 pages) and lists the links to
//! files of the chosen types. Same limits as Flux 1.x's `SiteGrabber`.

use std::collections::{BTreeSet, HashSet, VecDeque};

use url::Url;

use crate::Engine;
use crate::session::Session;

const MAX_PAGES: usize = 20;
const MAX_FILES: usize = 500;
const MAX_PAGE_BYTES: usize = 2_000_000;

fn same_site(root: &Url, link: &Url) -> bool {
    link.username().is_empty() && link.password().is_none() && link.origin() == root.origin()
}

/// Every `href="…"` value in a page, decoded for the common HTML entities.
fn hrefs(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(i) = lower[at..].find("href") {
        let mut j = at + i + 4;
        at = j;
        let rest = lower[j..].trim_start();
        j += lower[j..].len() - rest.len();
        if !rest.starts_with('=') {
            continue;
        }
        j += 1;
        j += lower[j..].len() - lower[j..].trim_start().len();
        let Some(quote) = html[j..].chars().next().filter(|c| *c == '"' || *c == '\'') else { continue };
        let start = j + 1;
        let Some(len) = html[start..].find(quote) else { continue };
        found.push(html[start..start + len].replace("&amp;", "&").replace("&#38;", "&").replace("&quot;", "\""));
        at = start + len;
    }
    found
}

fn extension(url: &Url) -> String {
    let name = url.path().rsplit('/').next().unwrap_or("");
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

impl Engine {
    /// Collects links to files whose extension is in `extensions` ("zip pdf mp4"), starting at `root`.
    pub async fn grab(&self, root: &Url, extensions: &str, session: Option<&Session>) -> Result<Vec<String>, String> {
        if !matches!(root.scheme(), "http" | "https") {
            return Err("The grabber needs an http:// or https:// address.".into());
        }
        let types: HashSet<String> = extensions.split([' ', ',', ';']).map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase()).filter(|e| !e.is_empty()).collect();
        if types.is_empty() {
            return Err("Name at least one file type, for example: zip pdf mp4.".into());
        }
        let client = self.client.read().unwrap().clone();
        let job = crate::Job::new(root.clone(), std::env::temp_dir().join("kestrel-grabber"));
        let mut pages = VecDeque::from([root.clone()]);
        let mut visited = HashSet::new();
        let mut files = BTreeSet::new();
        let mut first_error = None;
        while let Some(page) = pages.pop_front() {
            if visited.len() >= MAX_PAGES || files.len() >= MAX_FILES {
                break;
            }
            if !visited.insert(page.to_string()) {
                continue;
            }
            let headers = self.headers(&job, &page, session);
            let response = match client.get(page.clone()).headers(headers).send().await {
                Ok(r) if r.status().is_success() => r,
                Ok(r) => {
                    first_error.get_or_insert(format!("The site answered {} for {page}.", r.status()));
                    continue;
                }
                Err(e) => {
                    first_error.get_or_insert(format!("The site could not be reached: {e}"));
                    continue;
                }
            };
            // Redirects were followed; stay on the site that was asked for.
            if !same_site(root, response.url()) {
                continue;
            }
            let base = response.url().clone();
            let html = response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|t| t.starts_with("text/html"));
            if !html || response.content_length().is_some_and(|n| n as usize > MAX_PAGE_BYTES) {
                continue;
            }
            let Ok(body) = response.bytes().await else { continue };
            if body.len() > MAX_PAGE_BYTES {
                continue;
            }
            for href in hrefs(&String::from_utf8_lossy(&body)) {
                let Ok(mut link) = base.join(href.trim()) else { continue };
                link.set_fragment(None);
                if !same_site(root, &link) {
                    continue;
                }
                let ext = extension(&link);
                if types.contains(&ext) {
                    files.insert(link.to_string());
                    if files.len() >= MAX_FILES {
                        break;
                    }
                } else if pages.len() < 100 && matches!(ext.as_str(), "" | "html" | "htm" | "php") && !visited.contains(link.as_str()) {
                    pages.push_back(link);
                }
            }
        }
        match (files.is_empty(), first_error) {
            (true, Some(error)) => Err(error),
            _ => Ok(files.into_iter().collect()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_found_in_either_quote_style() {
        let html = r#"<a HREF = "a.zip">x</a> <a href='/dir/b.pdf?x=1&amp;y=2'>y</a> <link href=unquoted.css>"#;
        assert_eq!(hrefs(html), ["a.zip", "/dir/b.pdf?x=1&y=2"]);
        assert_eq!(extension(&Url::parse("https://a.test/x/File.ZIP?v=1").unwrap()), "zip");
    }
}
