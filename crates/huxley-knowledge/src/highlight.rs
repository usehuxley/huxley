//! Search snippets as safe HTML. `ts_headline` returns an article's text as
//! written, with the matches marked, and the apps render snippets as HTML:
//! so the text is escaped here and only the markers become `<b>`. (Marking
//! with `<b>` directly let HTML in an article's text through to the page.)

/// Marks a match's start and end (private-use characters, never markup).
pub(crate) const START: char = '\u{E000}';
pub(crate) const END: char = '\u{E001}';

/// `ts_headline`'s options, marking matches with [`START`] and [`END`].
pub(crate) fn options(max_words: u32, min_words: u32) -> String {
    format!(
        "StartSel={START}, StopSel={END}, MaxWords={max_words}, MinWords={min_words}, MaxFragments=1"
    )
}

/// The snippet escaped as HTML, with its matches in `<b>`.
pub fn to_html(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 16);
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            START => out.push_str("<b>"),
            END => out.push_str("</b>"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn article_text_is_escaped_and_only_matches_are_bold() {
        let raw =
            format!("<img src=x onerror=alert(1)> reset the {START}printer{END} & \"spooler\"");
        assert_eq!(
            to_html(&raw),
            "&lt;img src=x onerror=alert(1)&gt; reset the <b>printer</b> &amp; &quot;spooler&quot;"
        );
    }
}
