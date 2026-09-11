//! Minimal, XSS-safe markdown → HTML for assistant answers.
//!
//! Assistant text is model-generated, so it is untrusted input that ends up in
//! `inner_html`. Every raw-HTML event is dropped before serialising: pulldown-cmark
//! passes `<script>` and friends straight through by default, and nothing the model
//! writes needs raw HTML anyway.

use pulldown_cmark::{html, Event, Options, Parser};

/// Render markdown to HTML with all raw HTML removed.
pub fn render(markdown: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);

    let parser = Parser::new_ext(markdown, options).filter(|event| {
        !matches!(
            event,
            Event::Html(_) | Event::InlineHtml(_) | Event::FootnoteReference(_)
        )
    });

    let mut out = String::with_capacity(markdown.len() + markdown.len() / 2);
    html::push_html(&mut out, parser);
    out
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
    fn empty_input_is_empty_output() {
        assert!(render("").is_empty());
    }
}
