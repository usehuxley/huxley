//! Markdown to safe HTML: `CommonMark` with tables, strikethrough, task
//! lists and footnotes. Safe by construction rather than by sanitizing:
//! raw HTML in an article is shown as text, and links and images may only
//! point to http(s), mailto, tel or relative addresses. Everything else the
//! renderer escapes.

use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, html};

/// Whether a link or image address is safe to put in a page: no
/// `javascript:`, `data:` or other schemes, however disguised.
fn safe_url(url: &str) -> bool {
    // Browsers ignore whitespace and control characters in a scheme.
    let compact: String = url
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control())
        .collect::<String>()
        .to_ascii_lowercase();
    let scheme_end = compact.find(':');
    let path_start = compact.find(['/', '?', '#']);
    match (scheme_end, path_start) {
        // No scheme: a relative address (or a fragment).
        (None, _) => true,
        (Some(colon), Some(slash)) if slash < colon => true,
        (Some(colon), _) => matches!(&compact[..colon], "http" | "https" | "mailto" | "tel"),
    }
}

fn safe(url: CowStr<'_>) -> CowStr<'_> {
    if safe_url(&url) {
        url
    } else {
        CowStr::Borrowed("")
    }
}

/// Renders an article's markdown as HTML that's safe to show.
pub fn render(markdown: &str) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let events = Parser::new_ext(markdown, options).map(|event| match event {
        // Raw HTML is shown, not run.
        Event::Html(text) | Event::InlineHtml(text) => Event::Text(text),
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Link {
            link_type,
            dest_url: safe(dest_url),
            title,
            id,
        }),
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Image {
            link_type,
            dest_url: safe(dest_url),
            title,
            id,
        }),
        other => other,
    });
    let mut out = String::with_capacity(markdown.len() * 3 / 2);
    html::push_html(&mut out, events);
    out
}

#[cfg(test)]
mod tests {
    use super::{render, safe_url};

    #[test]
    fn markdown_becomes_html() {
        let html = render(
            "# Reset MFA\n\n1. Open **Entra**\n2. Run `Reset-Mfa`\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done\n\n[docs](https://learn.microsoft.com/) [ticket](/tickets/1)",
        );
        assert!(html.contains("<h1>Reset MFA</h1>"), "{html}");
        assert!(html.contains("<strong>Entra</strong>"));
        assert!(html.contains("<code>Reset-Mfa</code>"));
        assert!(html.contains("<table>"));
        assert!(html.contains("checked"));
        assert!(html.contains(r#"href="https://learn.microsoft.com/""#));
        assert!(html.contains(r#"href="/tickets/1""#));
    }

    #[test]
    fn raw_html_and_unsafe_links_are_harmless() {
        let html = render(
            "<script>alert(1)</script>\n\n<img src=x onerror=alert(1)> and <b onclick=x>inline</b>\n\n[click](javascript:alert(1)) [tab](java\tscript:alert(1)) ![img](data:text/html,x) [ok](https://example.com)",
        );
        assert!(!html.contains("<script"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
        assert!(!html.contains("<img src=x"), "{html}");
        assert!(!html.contains("<b onclick"), "{html}");
        // Unsafe addresses are emptied (one with a tab isn't even a link).
        assert!(
            !html.contains(r#"href="java"#) && !html.contains("javascript:alert"),
            "{html}"
        );
        assert!(!html.contains("data:text"), "{html}");
        assert!(html.contains(r#"href="https://example.com""#), "{html}");
    }

    #[test]
    fn only_safe_schemes_pass() {
        for ok in [
            "https://x.test",
            "http://x",
            "mailto:a@b.c",
            "tel:+1",
            "/a/b",
            "page#top",
            "#top",
            "a/b:c",
        ] {
            assert!(safe_url(ok), "{ok}");
        }
        for bad in [
            "javascript:x",
            "JavaScript:x",
            " javascript:x",
            "java\nscript:x",
            "data:x",
            "vbscript:x",
            "file:///etc",
        ] {
            assert!(!safe_url(bad), "{bad}");
        }
    }
}
