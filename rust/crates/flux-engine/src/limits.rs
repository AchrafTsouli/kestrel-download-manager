//! Remembered per-site connection limits and token-bucket speed limits.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use url::Url;

type Learned = Arc<dyn Fn(&str, usize) + Send + Sync>;

/// How many connections each site accepted after refusing more. Subdomains inherit a parent's limit.
#[derive(Clone, Default)]
pub struct HostLimits {
    limits: Arc<Mutex<HashMap<String, usize>>>,
    learned: Option<Learned>,
}

impl HostLimits {
    pub fn new(known: impl IntoIterator<Item = (String, usize)>) -> Self {
        let mut map: HashMap<String, usize> = known.into_iter().map(|(h, n)| (h.to_ascii_lowercase(), n.max(1))).collect();
        map.entry("gofile.io".into()).or_insert(2);
        HostLimits { limits: Arc::new(Mutex::new(map)), learned: None }
    }
    /// Called with (host, limit) whenever a limit is learned or lowered, so the app can save it.
    pub fn on_learned(mut self, callback: impl Fn(&str, usize) + Send + Sync + 'static) -> Self {
        self.learned = Some(Arc::new(callback));
        self
    }
    pub fn limit_for(&self, url: &Url) -> Option<usize> {
        let host = url.host_str()?.to_ascii_lowercase();
        let limits = self.limits.lock().unwrap();
        let mut candidate = host.as_str();
        loop {
            if let Some(limit) = limits.get(candidate) {
                return Some(*limit);
            }
            // Walk up to the parent domain, but never to a bare TLD.
            match candidate.split_once('.') {
                Some((_, parent)) if parent.contains('.') => candidate = parent,
                _ => return None,
            }
        }
    }
    pub fn learn(&self, url: &Url, connections: usize) {
        let Some(host) = url.host_str().map(str::to_ascii_lowercase) else { return };
        let connections = connections.max(1);
        if self.limit_for(url).is_some_and(|known| known <= connections) {
            return;
        }
        self.limits.lock().unwrap().insert(host.clone(), connections);
        if let Some(callback) = &self.learned {
            callback(&host, connections);
        }
    }
    pub fn snapshot(&self) -> HashMap<String, usize> {
        self.limits.lock().unwrap().clone()
    }
}

/// A token bucket shared by everything it limits. 0 bytes per second means unlimited.
#[derive(Clone)]
pub struct Limiter {
    inner: Arc<Mutex<Bucket>>,
}

struct Bucket {
    rate: u64,
    tokens: f64,
    updated: Instant,
}

impl std::fmt::Debug for Limiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Limiter({} B/s)", self.rate())
    }
}

impl Default for Limiter {
    fn default() -> Self {
        Limiter::new(0)
    }
}

impl Limiter {
    pub fn new(bytes_per_second: u64) -> Self {
        Limiter { inner: Arc::new(Mutex::new(Bucket { rate: bytes_per_second, tokens: 0.0, updated: Instant::now() })) }
    }
    pub fn set_rate(&self, bytes_per_second: u64) {
        let mut bucket = self.inner.lock().unwrap();
        bucket.rate = bytes_per_second;
        bucket.tokens = bucket.tokens.min(bytes_per_second as f64);
    }
    pub fn rate(&self) -> u64 {
        self.inner.lock().unwrap().rate
    }
    /// Waits until `bytes` may pass. Bursts are capped at one second's worth.
    pub async fn take(&self, bytes: usize) {
        loop {
            let wait = {
                let mut bucket = self.inner.lock().unwrap();
                if bucket.rate == 0 {
                    return;
                }
                let now = Instant::now();
                let rate = bucket.rate as f64;
                bucket.tokens = (bucket.tokens + now.duration_since(bucket.updated).as_secs_f64() * rate).min(rate);
                bucket.updated = now;
                if bucket.tokens >= bytes as f64 || bucket.tokens >= rate {
                    bucket.tokens -= bytes as f64;
                    return;
                }
                Duration::from_secs_f64(((bytes as f64).min(rate) - bucket.tokens) / rate)
            };
            tokio::time::sleep(wait.min(Duration::from_millis(250))).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subdomains_inherit_and_limits_only_go_down() {
        let learned = Arc::new(Mutex::new(Vec::new()));
        let seen = learned.clone();
        let limits = HostLimits::new([]).on_learned(move |h, n| seen.lock().unwrap().push((h.to_string(), n)));
        let site = Url::parse("https://files.example.com/a").unwrap();
        assert_eq!(limits.limit_for(&site), None);
        limits.learn(&site, 3);
        limits.learn(&site, 5);
        assert_eq!(limits.limit_for(&Url::parse("https://cdn.files.example.com/b").unwrap()), Some(3));
        assert_eq!(limits.limit_for(&Url::parse("https://example.com/").unwrap()), None);
        assert_eq!(limits.limit_for(&Url::parse("https://x.gofile.io/d").unwrap()), Some(2));
        assert_eq!(*learned.lock().unwrap(), vec![("files.example.com".to_string(), 3)]);
    }

    #[tokio::test]
    async fn limiter_holds_the_rate() {
        let limiter = Limiter::new(200_000);
        let start = Instant::now();
        for _ in 0..10 {
            limiter.take(50_000).await;
        }
        // 500 KB at 200 KB/s: about 2.5 s, minus the initial burst allowance.
        assert!(start.elapsed() >= Duration::from_millis(1400), "{:?}", start.elapsed());
    }
}
