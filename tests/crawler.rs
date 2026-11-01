//! Crawls a small site served from a local HTTP server: robots.txt, scope,
//! redirects, duplicates, non-HTML and error pages, and the page budget.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use search_engine::corpus::read_jsonl;
use search_engine::crawler::{CrawlConfig, DEFAULT_USER_AGENT, crawl};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

struct Page {
    status: &'static str,
    content_type: &'static str,
    extra_header: &'static str,
    body: &'static str,
}

fn html(body: &'static str) -> Page {
    Page {
        status: "200 OK",
        content_type: "text/html; charset=utf-8",
        extra_header: "",
        body,
    }
}

fn site() -> HashMap<&'static str, Page> {
    HashMap::from([
        (
            "/robots.txt",
            Page {
                content_type: "text/plain",
                ..html("User-agent: *\nDisallow: /docs/secret.html\n")
            },
        ),
        (
            "/docs/",
            html(
                r#"<html><head><title>Docs home</title></head><body><main>Welcome to the docs.
                <a href="a.html">A</a> <a href="a.html#part">A again</a> <a href="b.html">B</a>
                <a href="secret.html">secret</a> <a href="../outside.html">outside</a>
                <a href="https://other.example/">other host</a> <a href="logo.png">logo</a>
                </main></body></html>"#,
            ),
        ),
        (
            "/docs/a.html",
            html(
                r#"<title>A</title><main>Alpha page about ownership. <a href="index.html">home</a> <a href="moved.html">moved</a></main>"#,
            ),
        ),
        (
            "/docs/b.html",
            html(r#"<title>B</title><main>Beta page about borrowing. <a href="copy.html">copy</a></main>"#),
        ),
        (
            "/docs/copy.html",
            html(r#"<title>Copy</title><main>Beta page about borrowing. <a href="copy.html">copy</a></main>"#),
        ),
        (
            "/docs/moved.html",
            Page {
                status: "301 Moved Permanently",
                extra_header: "Location: /docs/c.html\r\n",
                ..html("")
            },
        ),
        (
            "/docs/c.html",
            html(r#"<title>C</title><main>Gamma page about lifetimes. <a href="missing.html">gone</a></main>"#),
        ),
        (
            "/docs/logo.png",
            Page {
                content_type: "image/png",
                ..html("PNG")
            },
        ),
        (
            "/docs/secret.html",
            html("<title>Secret</title><main>must never be fetched</main>"),
        ),
        ("/outside.html", html("<title>Outside</title><main>out of scope</main>")),
    ])
}

/// A minimal HTTP/1.1 server: one request per connection, then close.
async fn serve_site() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let requested = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&requested);
    let pages = Arc::new(site());
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let (pages, log) = (Arc::clone(&pages), Arc::clone(&log));
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                while !request.ends_with(b"\r\n\r\n") {
                    match stream.read(&mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buffer[..n]),
                    }
                }
                let request = String::from_utf8_lossy(&request);
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                log.lock().unwrap().push(path.clone());
                let not_found = Page {
                    status: "404 Not Found",
                    ..html("not found")
                };
                let page = pages.get(path.as_str()).unwrap_or(&not_found);
                let response = format!(
                    "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n{}",
                    page.status,
                    page.content_type,
                    page.body.len(),
                    page.extra_header,
                    page.body
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    (base, requested)
}

fn config(base: &str, max_pages: usize) -> CrawlConfig {
    CrawlConfig {
        seeds: vec![Url::parse(&format!("{base}/docs/")).unwrap()],
        max_pages,
        delay: Duration::from_millis(5),
        concurrency: 2,
        user_agent: DEFAULT_USER_AGENT.to_string(),
        excludes: Vec::new(),
        max_body_bytes: 1 << 20,
        request_timeout: Duration::from_secs(5),
    }
}

#[tokio::test]
async fn crawls_politely_within_scope() {
    let (base, requested) = serve_site().await;
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("corpus.jsonl");

    let report = crawl(config(&base, 100), &output).await.unwrap();

    let mut titles: Vec<String> = read_jsonl(&output).unwrap().into_iter().map(|doc| doc.title).collect();
    titles.sort();
    assert_eq!(titles, ["A", "B", "C", "Docs home"]);
    assert_eq!(report.pages_written, 4);
    assert_eq!(report.disallowed_by_robots, 1);
    assert_eq!(report.duplicate_content, 1, "copy.html repeats b.html");
    assert_eq!(report.not_html, 1);
    assert_eq!(report.http_errors, 1, "missing.html is a 404");

    let requested = requested.lock().unwrap().clone();
    assert_eq!(requested.first().map(String::as_str), Some("/robots.txt"));
    assert!(
        !requested.iter().any(|p| p == "/docs/secret.html"),
        "robots.txt disallows it"
    );
    assert!(
        !requested.iter().any(|p| p == "/outside.html"),
        "outside the seed directory"
    );
    assert_eq!(
        requested.iter().filter(|p| p.as_str() == "/docs/a.html").count(),
        1,
        "fragments are de-duplicated"
    );
    assert!(
        requested.iter().any(|p| p == "/docs/c.html"),
        "redirect target is followed"
    );
}

#[tokio::test]
async fn stops_at_the_page_budget() {
    let (base, _) = serve_site().await;
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("corpus.jsonl");
    let report = crawl(config(&base, 2), &output).await.unwrap();
    assert_eq!(report.pages_written, 2);
    assert_eq!(read_jsonl(&output).unwrap().len(), 2);
}

#[tokio::test]
async fn excluded_patterns_are_not_fetched() {
    let (base, requested) = serve_site().await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(&base, 100);
    config.excludes = vec!["b.html".into()];
    crawl(config, &dir.path().join("corpus.jsonl")).await.unwrap();
    assert!(!requested.lock().unwrap().iter().any(|p| p == "/docs/b.html"));
}
