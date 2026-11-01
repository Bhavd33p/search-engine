//! A polite, asynchronous web crawler that writes a JSONL corpus.
//!
//! Politeness rules it follows:
//! * fetches and obeys each host's `robots.txt` (including `Crawl-delay`;
//!   a host asking for more than a minute between requests is skipped);
//! * waits at least `delay` (or the site's `Crawl-delay`, if longer) between
//!   requests to the same host;
//! * backs off a host for ten delays after a 429 or 503 response;
//! * identifies itself with a descriptive User-Agent;
//! * stays inside the seeds' host and directory, and stops at `max_pages`.
//!
//! The frontier is breadth-first. Up to `concurrency` fetches run at once on
//! tokio tasks; the per-host throttle still spaces them out.

pub mod extract;
pub mod politeness;
pub mod urls;

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{CONTENT_TYPE, LOCATION};
use reqwest::{Client, StatusCode, redirect};
use serde::Serialize;
use thiserror::Error;
use tokio::task::JoinSet;
use tracing::{info, warn};
use url::Url;

use self::extract::extract;
use self::politeness::{HostThrottle, RobotsPolicy};
use self::urls::{Scope, normalize};
use crate::corpus::{CorpusError, Document, JsonlWriter};

/// Product token matched against `User-agent:` lines in robots.txt.
pub const ROBOTS_AGENT: &str = "search-engine-crawler";
pub const DEFAULT_USER_AGENT: &str =
    "search-engine-crawler/0.1 (educational search engine; +https://github.com/Bhavd33p/search-engine)";

#[derive(Debug, Clone)]
pub struct CrawlConfig {
    pub seeds: Vec<Url>,
    pub max_pages: usize,
    /// Minimum time between two requests to the same host.
    pub delay: Duration,
    pub concurrency: usize,
    pub user_agent: String,
    /// Skip URLs containing any of these substrings.
    pub excludes: Vec<String>,
    pub max_body_bytes: usize,
    pub request_timeout: Duration,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CrawlReport {
    pub pages_written: usize,
    pub requests: usize,
    pub disallowed_by_robots: usize,
    pub not_html: usize,
    pub duplicate_content: usize,
    pub http_errors: usize,
    pub fetch_errors: usize,
}

#[derive(Debug, Error)]
pub enum CrawlError {
    #[error("no valid seed URLs")]
    NoSeeds,
    #[error("building HTTP client: {0}")]
    Client(#[from] reqwest::Error),
    #[error(transparent)]
    Corpus(#[from] CorpusError),
}

/// Result of fetching one URL.
enum Fetched {
    Page {
        url: Url,
        title: String,
        text: String,
        links: Vec<Url>,
    },
    Redirect(Url),
    NotHtml,
    HttpError(StatusCode),
    Failed(String),
}

pub async fn crawl(config: CrawlConfig, output: &Path) -> Result<CrawlReport, CrawlError> {
    let seeds: Vec<Url> = config.seeds.iter().filter_map(normalize).collect();
    if seeds.is_empty() {
        return Err(CrawlError::NoSeeds);
    }
    let scope = Scope::from_seeds(&seeds);
    // Redirects are followed by hand, so their targets go through the same
    // robots, scope and duplicate checks as any other link.
    let client = Client::builder()
        .user_agent(config.user_agent.clone())
        .timeout(config.request_timeout)
        .redirect(redirect::Policy::none())
        .build()?;
    // robots.txt redirects (http → https, apex → www) are followed, as RFC 9309 asks.
    let robots_client = Client::builder()
        .user_agent(config.user_agent.clone())
        .timeout(config.request_timeout)
        .redirect(redirect::Policy::limited(5))
        .build()?;
    let throttle = Arc::new(HostThrottle::default());
    let mut robots: HashMap<String, Arc<RobotsPolicy>> = HashMap::new();

    let mut frontier: VecDeque<Url> = VecDeque::new();
    let mut seen: HashSet<String> = HashSet::new();
    for seed in seeds {
        if seen.insert(seed.to_string()) {
            frontier.push_back(seed);
        }
    }
    let mut content_hashes: HashSet<u64> = HashSet::new();
    let mut writer = JsonlWriter::create(output)?;
    let mut report = CrawlReport::default();
    let mut in_flight: JoinSet<(Url, Fetched)> = JoinSet::new();

    loop {
        while in_flight.len() < config.concurrency.max(1) && report.pages_written + in_flight.len() < config.max_pages {
            let Some(url) = frontier.pop_front() else {
                break;
            };
            let Some(host) = url.host_str().map(str::to_owned) else {
                continue;
            };
            let policy = match robots.get(&host) {
                Some(policy) => Arc::clone(policy),
                None => {
                    let policy = Arc::new(fetch_robots(&robots_client, &throttle, &url, &config).await);
                    robots.insert(host.clone(), Arc::clone(&policy));
                    policy
                }
            };
            if !policy.allows(url.as_str()) {
                report.disallowed_by_robots += 1;
                continue;
            }
            let delay = host_delay(&policy, config.delay);
            let (client, throttle, max_body) = (client.clone(), Arc::clone(&throttle), config.max_body_bytes);
            in_flight.spawn(async move {
                throttle.wait_turn(&host, delay).await;
                let fetched = fetch(&client, &url, max_body).await;
                (url, fetched)
            });
        }

        let Some(joined) = in_flight.join_next().await else {
            break; // nothing running and nothing left to start
        };
        let (url, fetched) = match joined {
            Ok(result) => result,
            Err(error) => {
                warn!(%error, "fetch task failed");
                report.fetch_errors += 1;
                continue;
            }
        };
        report.requests += 1;

        let mut enqueue = |link: &Url| {
            let Some(link) = normalize(link) else {
                return;
            };
            let key = link.to_string();
            if scope.contains(&link)
                && !config.excludes.iter().any(|pattern| key.contains(pattern.as_str()))
                && seen.insert(key)
            {
                frontier.push_back(link);
            }
        };
        match fetched {
            Fetched::Page {
                url: page_url,
                title,
                text,
                links,
            } => {
                links.iter().for_each(&mut enqueue);
                if text.is_empty() || !content_hashes.insert(hash_text(&text)) {
                    report.duplicate_content += 1;
                    continue;
                }
                writer.write(&Document {
                    id: page_url.to_string(),
                    url: page_url.to_string(),
                    title,
                    body: text,
                })?;
                report.pages_written += 1;
                info!(pages = report.pages_written, max = config.max_pages, url = %page_url, "saved page");
            }
            Fetched::Redirect(target) => enqueue(&target),
            Fetched::NotHtml => report.not_html += 1,
            Fetched::HttpError(status) => {
                warn!(%url, %status, "HTTP error");
                report.http_errors += 1;
                if matches!(status, StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE)
                    && let Some(host) = url.host_str()
                {
                    let delay = robots
                        .get(host)
                        .map_or(config.delay, |policy| host_delay(policy, config.delay));
                    throttle.back_off(host, delay * BACKOFF_DELAYS).await;
                }
            }
            Fetched::Failed(error) => {
                warn!(%url, %error, "fetch failed");
                report.fetch_errors += 1;
            }
        }
        if report.pages_written >= config.max_pages {
            break;
        }
    }
    in_flight.abort_all();
    writer.finish()?;
    Ok(report)
}

/// After a 429 or 503, the host gets no requests for this many delays.
const BACKOFF_DELAYS: u32 = 10;

/// Delay between requests to one host: ours, or the site's Crawl-delay if longer.
fn host_delay(policy: &RobotsPolicy, configured: Duration) -> Duration {
    policy
        .crawl_delay()
        .map_or(configured, |requested| requested.max(configured))
}

async fn fetch_robots(client: &Client, throttle: &HostThrottle, url: &Url, config: &CrawlConfig) -> RobotsPolicy {
    let Ok(robots_url) = url.join("/robots.txt") else {
        return RobotsPolicy::DisallowAll;
    };
    if let Some(host) = url.host_str() {
        throttle.wait_turn(host, config.delay).await;
    }
    let response = match client.get(robots_url.clone()).send().await {
        Ok(response) => response,
        Err(error) => {
            warn!(%robots_url, %error, "robots.txt unreachable; not crawling this host");
            return RobotsPolicy::DisallowAll;
        }
    };
    let status = response.status().as_u16();
    let body = response.bytes().await.unwrap_or_default();
    let policy = RobotsPolicy::from_response(Some(status), &body, ROBOTS_AGENT);
    info!(%robots_url, status, crawl_delay = ?policy.crawl_delay(), "fetched robots.txt");
    if let (Some(host), Some(requested)) = (url.host_str(), policy.crawl_delay())
        && requested > config.delay
    {
        // The robots.txt request itself was spaced by our own delay; make the
        // first page wait for the site's longer Crawl-delay too.
        throttle.back_off(host, requested).await;
    }
    policy
}

async fn fetch(client: &Client, url: &Url, max_body_bytes: usize) -> Fetched {
    let mut response = match client.get(url.clone()).send().await {
        Ok(response) => response,
        Err(error) => return Fetched::Failed(error.to_string()),
    };
    let status = response.status();
    if status.is_redirection() {
        let target = response.headers().get(LOCATION).and_then(|value| value.to_str().ok());
        return match target.and_then(|location| url.join(location).ok()) {
            Some(target) => Fetched::Redirect(target),
            None => Fetched::HttpError(status),
        };
    }
    if !status.is_success() {
        return Fetched::HttpError(status);
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !(content_type.starts_with("text/html") || content_type.starts_with("application/xhtml+xml")) {
        return Fetched::NotHtml;
    }

    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                body.extend_from_slice(&chunk);
                if body.len() > max_body_bytes {
                    return Fetched::Failed(format!("page larger than {max_body_bytes} bytes"));
                }
            }
            Ok(None) => break,
            Err(error) => return Fetched::Failed(error.to_string()),
        }
    }
    let html = String::from_utf8_lossy(&body);
    let page = extract(&html, url);
    Fetched::Page {
        url: url.clone(),
        title: page.title,
        text: page.text,
        links: page.links,
    }
}

fn hash_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}
