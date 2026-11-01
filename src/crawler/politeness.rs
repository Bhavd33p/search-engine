//! robots.txt rules and per-host request spacing.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use texting_robots::Robot;
use tokio::time::{Instant, sleep_until};
use tracing::warn;

/// A site asking for a longer `Crawl-delay` than this is not crawled at all:
/// honouring an hour-long delay would stall the crawl, and ignoring it would be rude.
pub const MAX_CRAWL_DELAY: Duration = Duration::from_secs(60);

/// What a host's robots.txt lets us do.
pub enum RobotsPolicy {
    AllowAll,
    DisallowAll,
    Rules(Box<Robot>),
}

impl RobotsPolicy {
    /// Policy for a robots.txt response, following RFC 9309:
    /// a missing file (4xx) allows everything; an unreachable one (5xx, 429
    /// "too many requests", or a network error) disallows everything, since
    /// we cannot know the rules.
    pub fn from_response(status: Option<u16>, body: &[u8], agent: &str) -> Self {
        match status {
            Some(200..=299) => match Robot::new(agent, body) {
                Ok(robot) => Self::checked_rules(robot),
                Err(_) => RobotsPolicy::AllowAll, // unparsable file: treat as no rules
            },
            Some(429) => RobotsPolicy::DisallowAll,
            Some(400..=499) => RobotsPolicy::AllowAll,
            _ => RobotsPolicy::DisallowAll,
        }
    }

    fn checked_rules(robot: Robot) -> Self {
        match robot.delay {
            // `inf`, `NaN` or an enormous value: we cannot honour it.
            Some(seconds) if !(seconds.is_finite() && seconds <= MAX_CRAWL_DELAY.as_secs_f32()) => {
                warn!(
                    crawl_delay = seconds,
                    "robots.txt asks for an unusable Crawl-delay; skipping this host"
                );
                RobotsPolicy::DisallowAll
            }
            _ => RobotsPolicy::Rules(Box::new(robot)),
        }
    }

    pub fn allows(&self, url: &str) -> bool {
        match self {
            RobotsPolicy::AllowAll => true,
            RobotsPolicy::DisallowAll => false,
            RobotsPolicy::Rules(robot) => robot.allowed(url),
        }
    }

    /// The site's requested `Crawl-delay`, if any.
    pub fn crawl_delay(&self) -> Option<Duration> {
        match self {
            RobotsPolicy::Rules(robot) => robot
                .delay
                .and_then(|secs| Duration::try_from_secs_f32(secs.max(0.0)).ok()),
            _ => None,
        }
    }
}

/// Spaces requests to the same host at least `delay` apart, even when several
/// fetches run concurrently. Each caller reserves the next free slot for its
/// host under the lock, then sleeps until that slot without holding the lock.
#[derive(Default)]
pub struct HostThrottle {
    next_slot: Mutex<HashMap<String, Instant>>,
}

impl HostThrottle {
    pub async fn wait_turn(&self, host: &str, delay: Duration) {
        let slot = {
            let mut next_slot = self.next_slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let now = Instant::now();
            let slot = next_slot.get(host).map_or(now, |&reserved| reserved.max(now));
            next_slot.insert(host.to_string(), slot + delay);
            slot
        };
        sleep_until(slot).await;
    }

    /// Keeps the host idle for at least `pause` from now (after a 429/503, or
    /// to honour a Crawl-delay learned after the robots.txt request).
    pub async fn back_off(&self, host: &str, pause: Duration) {
        let mut next_slot = self.next_slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let resume_at = Instant::now() + pause;
        let slot = next_slot.entry(host.to_string()).or_insert(resume_at);
        *slot = (*slot).max(resume_at);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    const ROBOTS: &[u8] = b"User-agent: *\nDisallow: /private/\nCrawl-delay: 2\n\nUser-agent: badbot\nDisallow: /\n";

    #[test]
    fn follows_robots_rules_for_our_agent() {
        let policy = RobotsPolicy::from_response(Some(200), ROBOTS, "search-engine-crawler");
        assert!(policy.allows("https://example.org/book/ch01.html"));
        assert!(!policy.allows("https://example.org/private/notes.html"));
        assert_eq!(policy.crawl_delay(), Some(Duration::from_secs(2)));

        let blocked = RobotsPolicy::from_response(Some(200), ROBOTS, "badbot");
        assert!(!blocked.allows("https://example.org/book/ch01.html"));
    }

    #[test]
    fn missing_robots_allows_and_server_errors_disallow() {
        assert!(RobotsPolicy::from_response(Some(404), b"", "a").allows("https://example.org/x"));
        assert!(!RobotsPolicy::from_response(Some(503), b"", "a").allows("https://example.org/x"));
        assert!(!RobotsPolicy::from_response(Some(429), b"", "a").allows("https://example.org/x"));
        assert!(!RobotsPolicy::from_response(None, b"", "a").allows("https://example.org/x"));
    }

    #[test]
    fn unusable_crawl_delays_skip_the_host_instead_of_panicking() {
        for delay in ["inf", "1e39", "100000"] {
            let robots = format!("User-agent: *\nCrawl-delay: {delay}\n");
            let policy = RobotsPolicy::from_response(Some(200), robots.as_bytes(), "a");
            assert!(!policy.allows("https://example.org/x"), "Crawl-delay: {delay}");
            assert_eq!(policy.crawl_delay(), None);
        }
        let policy = RobotsPolicy::from_response(Some(200), b"User-agent: *\nCrawl-delay: 60\n", "a");
        assert_eq!(policy.crawl_delay(), Some(MAX_CRAWL_DELAY));
        // The parser drops a NaN delay, so our own delay applies.
        let policy = RobotsPolicy::from_response(Some(200), b"User-agent: *\nCrawl-delay: NaN\n", "a");
        assert!(policy.allows("https://example.org/x"));
        assert_eq!(policy.crawl_delay(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn same_host_requests_are_spaced_by_the_delay() {
        let throttle = Arc::new(HostThrottle::default());
        let start = Instant::now();
        let delay = Duration::from_secs(1);

        let mut tasks = Vec::new();
        for _ in 0..3 {
            let throttle = Arc::clone(&throttle);
            tasks.push(tokio::spawn(async move {
                throttle.wait_turn("example.org", delay).await;
                Instant::now()
            }));
        }
        let mut times = Vec::new();
        for task in tasks {
            times.push(task.await.unwrap() - start);
        }
        times.sort();
        assert_eq!(times, [Duration::ZERO, delay, delay * 2]);

        // Another host is not held back by example.org's queue.
        let before = Instant::now();
        throttle.wait_turn("other.example", delay).await;
        assert_eq!(Instant::now(), before);
    }

    #[tokio::test(start_paused = true)]
    async fn back_off_delays_the_next_request() {
        let throttle = HostThrottle::default();
        let start = Instant::now();
        throttle.wait_turn("example.org", Duration::from_secs(1)).await;
        throttle.back_off("example.org", Duration::from_secs(10)).await;
        throttle.wait_turn("example.org", Duration::from_secs(1)).await;
        assert_eq!(Instant::now() - start, Duration::from_secs(10));

        // A shorter back-off never shortens an existing wait.
        throttle.back_off("example.org", Duration::from_secs(1)).await;
        throttle.wait_turn("example.org", Duration::from_secs(1)).await;
        assert_eq!(Instant::now() - start, Duration::from_secs(11));
    }
}
