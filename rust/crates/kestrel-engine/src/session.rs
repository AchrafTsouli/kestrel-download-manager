//! What the browser hands over with a download: cookies, referring page, browser identity, the
//! request headers it sent, and manual credentials. Same rules as Flux 1.x (`DownloadSession.cs`).

use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    #[serde(default = "root")]
    pub path: String,
    #[serde(default = "yes")]
    pub host_only: bool,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub expires_unix_seconds: Option<f64>,
}
fn root() -> String {
    "/".into()
}
fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

/// Browser headers that are replayed. Cookie, Host and the like never are.
pub const ALLOWED_HEADERS: &[&str] = &[
    "accept", "accept-language", "sec-fetch-site", "sec-fetch-mode", "sec-fetch-dest", "sec-fetch-user",
    "sec-ch-ua", "sec-ch-ua-mobile", "sec-ch-ua-platform", "upgrade-insecure-requests", "x-requested-with",
];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// `scheme://host[:port]/` the session belongs to.
    pub origin: String,
    #[serde(default)]
    pub referer: Option<String>,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub authorization: Option<String>,
    #[serde(default)]
    pub cookies: Vec<Cookie>,
    #[serde(default)]
    pub headers: Vec<Header>,
}

fn clean(value: &str) -> bool {
    !value.chars().any(|c| c.is_control())
}

/// `scheme://host[:port]/` for any address Kestrel downloads from.
pub fn origin_of(url: &Url) -> String {
    match url.port() {
        Some(port) => format!("{}://{}:{port}/", url.scheme(), url.host_str().unwrap_or_default()),
        None => format!("{}://{}/", url.scheme(), url.host_str().unwrap_or_default()),
    }
}

pub fn domain_matches(host: &str, domain: &str, host_only: bool) -> bool {
    let domain = domain.trim_start_matches('.').to_ascii_lowercase();
    let host = host.to_ascii_lowercase();
    !domain.is_empty() && (host == domain || !host_only && host.ends_with(&format!(".{domain}")))
}

fn path_matches(path: &str, cookie_path: &str) -> bool {
    path == cookie_path || path.starts_with(cookie_path) && (cookie_path.ends_with('/') || path.as_bytes().get(cookie_path.len()) == Some(&b'/'))
}

impl Session {
    pub fn for_url(url: &Url) -> Session {
        Session { origin: origin_of(url), ..Default::default() }
    }

    /// Rejects sessions that do not belong to `url` or carry unsafe values.
    pub fn validate(&self, url: &Url) -> Result<()> {
        let bad = |what: &str| Err(Error::protocol(format!("Invalid download session: {what}.")));
        let Ok(origin) = Url::parse(&self.origin) else { return bad("origin") };
        // Compared by scheme, host and port: SFTP addresses have no web origin of their own.
        let same = origin.scheme() == url.scheme() && origin.host_str() == url.host_str() && origin.port_or_known_default() == url.port_or_known_default();
        if !matches!(origin.scheme(), "http" | "https" | "ftp" | "ftps" | "sftp") || !origin.username().is_empty() || origin.path() != "/" || origin.query().is_some() || !same {
            return bad("the origin must match the download address");
        }
        if self.cookies.len() > 200 || self.cookies.iter().map(|c| c.name.len() + c.value.len()).sum::<usize>() > 24_000 {
            return bad("too many cookies");
        }
        let host = url.host_str().unwrap_or_default();
        for cookie in &self.cookies {
            if cookie.name.is_empty() || cookie.name.chars().any(|c| c.is_control() || c.is_whitespace() || "=;,".contains(c))
                || cookie.value.chars().any(|c| c.is_control() || c == ';')
                || !domain_matches(host, &cookie.domain, cookie.host_only) || !cookie.path.starts_with('/')
            {
                return bad("cookie");
            }
        }
        for value in [&self.referer, &self.user_agent, &self.authorization].into_iter().flatten() {
            if value.len() > 8192 || !clean(value) {
                return bad("header");
            }
        }
        if let Some(referer) = &self.referer {
            match Url::parse(referer) {
                Ok(page) if matches!(page.scheme(), "http" | "https") && page.username().is_empty() => {}
                _ => return bad("referring page"),
            }
        }
        if self.headers.len() > 16 || self.headers.iter().any(|h| !ALLOWED_HEADERS.contains(&h.name.to_ascii_lowercase().as_str()) || h.value.len() > 2048 || !clean(&h.value)) {
            return bad("browser header");
        }
        Ok(())
    }

    /// Headers for a request to `target`: identity, referer, credentials (same origin only), cookies and
    /// the browser's own headers, which override the defaults.
    pub fn apply(&self, target: &Url, headers: &mut HeaderMap) {
        let set = |headers: &mut HeaderMap, name: &str, value: &str| {
            if let (Ok(name), Ok(value)) = (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(value)) {
                headers.insert(name, value);
            }
        };
        if let Some(agent) = &self.user_agent {
            set(headers, "user-agent", agent);
        }
        let same_origin = Url::parse(&self.origin).is_ok_and(|o| o.origin() == target.origin());
        if same_origin
            && let Some(auth) = &self.authorization {
                set(headers, "authorization", auth);
            }
        if let Some(referer) = &self.referer {
            // Never leak an https referer to an http request.
            if !(referer.starts_with("https:") && target.scheme() == "http") {
                set(headers, "referer", referer.split('#').next().unwrap_or(referer));
            }
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let host = target.host_str().unwrap_or_default();
        let mut cookies: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|c| domain_matches(host, &c.domain, c.host_only) && (!c.secure || target.scheme() == "https")
                && c.expires_unix_seconds.is_none_or(|e| e > now) && path_matches(target.path(), &c.path))
            .collect();
        cookies.sort_by_key(|c| std::cmp::Reverse(c.path.len()));
        if !cookies.is_empty() {
            let line = cookies.iter().map(|c| format!("{}={}", c.name, c.value)).collect::<Vec<_>>().join("; ");
            set(headers, "cookie", &line);
        }
        for header in &self.headers {
            set(headers, &header.name, &header.value);
        }
    }

    /// The headers a browser sends when an address is opened directly (typed or clicked).
    pub fn navigation_headers(from_page: bool) -> Vec<Header> {
        let h = |name: &str, value: &str| Header { name: name.into(), value: value.into() };
        vec![
            h("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
            h("Sec-Fetch-Dest", "document"),
            h("Sec-Fetch-Mode", "navigate"),
            h("Sec-Fetch-Site", if from_page { "same-site" } else { "none" }),
            h("Sec-Fetch-User", "?1"),
            h("Upgrade-Insecure-Requests", "1"),
        ]
    }

    /// This session with navigation headers replacing any of the same name.
    pub fn as_navigation(&self) -> Session {
        let navigation = Session::navigation_headers(self.referer.is_some());
        let mut copy = self.clone();
        copy.headers.retain(|h| !navigation.iter().any(|n| n.name.eq_ignore_ascii_case(&h.name)));
        copy.headers.extend(navigation);
        copy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn origin_must_match_and_unsafe_values_are_refused() {
        let target = url("https://files.test/a.zip");
        assert!(Session::for_url(&target).validate(&target).is_ok());
        assert!(Session { origin: "https://evil.test/".into(), ..Default::default() }.validate(&target).is_err());
        let bad_header = Session { headers: vec![Header { name: "Cookie".into(), value: "x=1".into() }], ..Session::for_url(&target) };
        assert!(bad_header.validate(&target).is_err());
        let injection = Session { headers: vec![Header { name: "Accept".into(), value: "a\r\nX: 1".into() }], ..Session::for_url(&target) };
        assert!(injection.validate(&target).is_err());
    }

    #[test]
    fn cookies_follow_domain_path_secure_and_browser_headers_override() {
        let target = url("https://cdn.files.test/dl/a.zip");
        let session = Session {
            origin: "https://cdn.files.test/".into(),
            cookies: vec![
                Cookie { name: "sid".into(), value: "1".into(), domain: ".files.test".into(), host_only: false, secure: true, ..Default::default() },
                Cookie { name: "other".into(), value: "2".into(), domain: "other.test".into(), ..Default::default() },
                Cookie { name: "deep".into(), value: "3".into(), domain: "cdn.files.test".into(), path: "/dl".into(), ..Default::default() },
            ],
            headers: vec![Header { name: "Accept-Language".into(), value: "fr-FR".into() }],
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        headers.insert("accept-language", HeaderValue::from_static("en-US"));
        session.apply(&target, &mut headers);
        assert_eq!(headers["cookie"], "deep=3; sid=1");
        assert_eq!(headers["accept-language"], "fr-FR");
        let mut plain = HeaderMap::new();
        session.apply(&url("http://cdn.files.test/dl/a.zip"), &mut plain);
        assert_eq!(plain["cookie"], "deep=3", "secure cookies stay off plain http");
    }

    #[test]
    fn navigation_form_replaces_same_named_headers() {
        let session = Session { headers: vec![Header { name: "sec-fetch-mode".into(), value: "no-cors".into() }], ..Default::default() };
        let nav = session.as_navigation();
        assert_eq!(nav.headers.iter().filter(|h| h.name.eq_ignore_ascii_case("sec-fetch-mode")).count(), 1);
        assert!(nav.headers.iter().any(|h| h.value == "navigate"));
    }
}
