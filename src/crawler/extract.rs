//! Pulls the title, the visible text and the outgoing links out of an HTML page.

use ego_tree::iter::Edge;
use scraper::{Html, Node, Selector};
use url::Url;

/// Subtrees that are never visible text, or are site chrome repeated on every page.
const SKIPPED_ELEMENTS: &[&str] = &[
    "script", "style", "noscript", "template", "svg", "nav", "header", "footer", "aside", "button", "form", "iframe",
];

/// Elements that separate words, so `<p>a</p><p>b</p>` reads "a b", not "ab".
const BLOCK_ELEMENTS: &[&str] = &[
    "p",
    "div",
    "br",
    "li",
    "ul",
    "ol",
    "dl",
    "dt",
    "dd",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "pre",
    "blockquote",
    "table",
    "tr",
    "td",
    "th",
    "section",
    "article",
    "main",
    "hr",
    "figure",
    "figcaption",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedPage {
    pub title: String,
    pub text: String,
    pub links: Vec<Url>,
}

pub fn extract(html: &str, page_url: &Url) -> ExtractedPage {
    let document = Html::parse_document(html);
    let base = select_first(&document, "base[href]")
        .and_then(|base| base.attr("href"))
        .and_then(|href| page_url.join(href).ok())
        .unwrap_or_else(|| page_url.clone());

    let title = select_first(&document, "title")
        .or_else(|| select_first(&document, "h1"))
        .map(|element| collapse_whitespace(&element.text().collect::<String>()))
        .unwrap_or_default();

    // Prefer the main content area, so navigation menus repeated on every
    // page do not drown out what the page is actually about.
    let content_root = ["main", "article", "[role=main]", "body"]
        .iter()
        .find_map(|selector| select_first(&document, selector))
        .unwrap_or_else(|| document.root_element());
    let text = visible_text(*content_root);

    let links = match Selector::parse("a[href]") {
        Ok(selector) => document
            .select(&selector)
            .filter_map(|anchor| anchor.attr("href"))
            .filter_map(|href| base.join(href.trim()).ok())
            .collect(),
        Err(_) => Vec::new(),
    };

    ExtractedPage { title, text, links }
}

fn select_first<'a>(document: &'a Html, selector: &str) -> Option<scraper::ElementRef<'a>> {
    let selector = Selector::parse(selector).ok()?;
    document.select(&selector).next()
}

fn visible_text(root: ego_tree::NodeRef<'_, Node>) -> String {
    let mut text = String::new();
    // Depth inside a skipped subtree; 0 means we are collecting text.
    let mut skip_depth = 0usize;
    for edge in root.traverse() {
        match edge {
            Edge::Open(node) => match node.value() {
                Node::Element(element) if skip_depth > 0 || SKIPPED_ELEMENTS.contains(&element.name()) => {
                    skip_depth += 1;
                }
                Node::Element(element) if BLOCK_ELEMENTS.contains(&element.name()) => text.push(' '),
                Node::Text(chunk) if skip_depth == 0 => text.push_str(chunk),
                _ => {}
            },
            Edge::Close(node) => match node.value() {
                Node::Element(_) if skip_depth > 0 => skip_depth -= 1,
                Node::Element(element) if BLOCK_ELEMENTS.contains(&element.name()) => text.push(' '),
                _ => {}
            },
        }
    }
    collapse_whitespace(&text)
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<!DOCTYPE html>
<html><head><title>
  What is Ownership?
</title><script>var x = "never indexed";</script></head>
<body>
  <nav><a href="ch01.html">Chapter 1</a> table of contents</nav>
  <main>
    <h1>Ownership</h1><p>Each value has an <em>owner</em>.</p><p>Second paragraph</p>
    <style>.x { color: red }</style>
    <pre><code>let s = String::from("hello");</code></pre>
    <a href="ch05.html#methods">methods</a>
    <a href="https://other.example/page">elsewhere</a>
    <a href="mailto:a@b.c">mail</a>
  </main>
  <footer>Copyright notice</footer>
</body></html>"#;

    fn page_url() -> Url {
        Url::parse("https://example.org/book/ch04.html").unwrap()
    }

    #[test]
    fn extracts_title_and_main_text_only() {
        let page = extract(PAGE, &page_url());
        assert_eq!(page.title, "What is Ownership?");
        assert_eq!(
            page.text,
            "Ownership Each value has an owner. Second paragraph let s = String::from(\"hello\"); methods elsewhere mail"
        );
    }

    #[test]
    fn resolves_links_against_the_page_url() {
        let page = extract(PAGE, &page_url());
        let links: Vec<&str> = page.links.iter().map(Url::as_str).collect();
        assert_eq!(
            links,
            [
                "https://example.org/book/ch01.html",
                "https://example.org/book/ch05.html#methods",
                "https://other.example/page",
                "mailto:a@b.c",
            ]
        );
    }

    #[test]
    fn honours_the_base_element() {
        let html =
            r#"<html><head><base href="https://cdn.example/docs/"></head><body><a href="a.html">a</a></body></html>"#;
        let page = extract(html, &page_url());
        assert_eq!(page.links[0].as_str(), "https://cdn.example/docs/a.html");
    }

    #[test]
    fn falls_back_to_body_and_h1() {
        let html = "<html><body><h1>Heading</h1><div>one</div><div>two</div><nav>menu</nav></body></html>";
        let page = extract(html, &page_url());
        assert_eq!(page.title, "Heading");
        assert_eq!(page.text, "Heading one two");
    }
}
