//! URL normalisation and crawl scope.

use url::Url;

/// Canonical form used for de-duplication, or `None` for URLs we never crawl.
///
/// * only `http`/`https`;
/// * no `#fragment` (same document);
/// * `/dir/index.html` → `/dir/` (same document on static sites);
/// * query parameters sorted, empty query removed.
///
/// Lower-casing the scheme and host and dropping default ports is already done
/// by the `url` crate's parser.
pub fn normalize(url: &Url) -> Option<Url> {
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    let mut url = url.clone();
    url.set_fragment(None);
    if let Some(directory) = url.path().strip_suffix("index.html").map(str::to_owned)
        && directory.ends_with('/')
    {
        url.set_path(&directory);
    }
    match url.query() {
        Some("") => url.set_query(None),
        Some(_) => {
            let mut pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
            pairs.sort();
            url.query_pairs_mut().clear().extend_pairs(pairs);
        }
        None => {}
    }
    Some(url)
}

/// The part of the web a crawl may visit: same host as a seed, and under the
/// seed's directory. Seeding `https://doc.rust-lang.org/book/` keeps the crawl
/// inside the book instead of wandering into the standard library docs.
#[derive(Debug, Clone)]
pub struct Scope {
    prefixes: Vec<(String, String)>,
}

impl Scope {
    pub fn from_seeds(seeds: &[Url]) -> Self {
        let prefixes = seeds
            .iter()
            .filter_map(|seed| {
                let host = seed.host_str()?.to_string();
                let path = seed.path();
                let directory = &path[..path.rfind('/').map_or(0, |i| i + 1)];
                Some((host, directory.to_string()))
            })
            .collect();
        Self { prefixes }
    }

    pub fn contains(&self, url: &Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        self.prefixes
            .iter()
            .any(|(scope_host, prefix)| scope_host == host && url.path().starts_with(prefix.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(input: &str) -> Option<String> {
        normalize(&Url::parse(input).unwrap()).map(String::from)
    }

    #[test]
    fn strips_fragments_index_pages_and_default_ports() {
        assert_eq!(
            norm("https://Example.org:443/a/b.html#part").as_deref(),
            Some("https://example.org/a/b.html")
        );
        assert_eq!(
            norm("https://example.org/book/index.html").as_deref(),
            Some("https://example.org/book/")
        );
        assert_eq!(norm("http://example.org:80/").as_deref(), Some("http://example.org/"));
        assert_eq!(
            norm("https://example.org/notindex.html").as_deref(),
            Some("https://example.org/notindex.html")
        );
    }

    #[test]
    fn sorts_query_parameters() {
        assert_eq!(
            norm("https://example.org/s?b=2&a=1").as_deref(),
            Some("https://example.org/s?a=1&b=2")
        );
        assert_eq!(norm("https://example.org/s?").as_deref(), Some("https://example.org/s"));
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert_eq!(norm("mailto:someone@example.org"), None);
        assert_eq!(norm("ftp://example.org/file"), None);
        assert_eq!(norm("javascript:void(0)"), None);
    }

    #[test]
    fn scope_is_host_plus_seed_directory() {
        let scope = Scope::from_seeds(&[Url::parse("https://doc.rust-lang.org/book/").unwrap()]);
        let inside = Url::parse("https://doc.rust-lang.org/book/ch01-00-getting-started.html").unwrap();
        let other_path = Url::parse("https://doc.rust-lang.org/std/index.html").unwrap();
        let other_host = Url::parse("https://github.com/book/").unwrap();
        assert!(scope.contains(&inside));
        assert!(!scope.contains(&other_path));
        assert!(!scope.contains(&other_host));

        let file_seed = Scope::from_seeds(&[Url::parse("https://example.org/docs/start.html").unwrap()]);
        assert!(file_seed.contains(&Url::parse("https://example.org/docs/next.html").unwrap()));
    }
}
