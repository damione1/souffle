//! Minimal block-level Markdown renderer for the What's New / Update
//! Available dialogs (AC4).
//!
//! Slint's `@markdown()` builtin (confirmed real: `BuiltinFunction::
//! ParseMarkdown` in the compiler) only accepts a string *literal*, not a
//! runtime expression - dynamic release-note text (from `check_for_updates`)
//! cannot go through it. This module does block-level parsing instead
//! (headings, bullet lists, paragraphs, and a whole-line link, which covers
//! the shape CLAUDE.md's own release-note template produces: `## Fixed`
//! sections with `-` bullets and a trailing release-page link) and each
//! block renders with real Slint styling - a genuine renderer, not a
//! plain-text dump.
//!
//! Known gap: inline emphasis (`**bold**` mid-sentence) and inline links
//! (`[text](url)`) are not styled/clickable - Slint's `Text` has no mixed-
//! run rich text short of `@markdown`'s literal-only builtin. `**bold**`
//! markers are stripped to plain text; inline links keep their link text
//! and drop the URL. A whole line that is only a link (the common case in
//! release notes) still renders as a real clickable link.

use crate::{MarkdownBlock, MarkdownBlockKind};

fn strip_inline_emphasis(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if (c == '*' || c == '_') && chars.peek() == Some(&c) {
            chars.next();
            continue;
        }
        out.push(c);
    }
    out
}

/// Drops a `[label](url)` inline link down to its label text.
fn strip_inline_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find(']') else {
            break;
        };
        let close = open + close;
        let Some(paren_open) = rest[close..].find('(') else {
            out.push_str(&rest[..close + 1]);
            rest = &rest[close + 1..];
            continue;
        };
        let paren_open = close + paren_open;
        let Some(paren_close) = rest[paren_open..].find(')') else {
            out.push_str(&rest[..close + 1]);
            rest = &rest[close + 1..];
            continue;
        };
        let paren_close = paren_open + paren_close;
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..close]);
        rest = &rest[paren_close + 1..];
    }
    out.push_str(rest);
    out
}

fn is_bare_link(line: &str) -> bool {
    line.starts_with("https://") || line.starts_with("http://")
}

pub fn render_blocks(markdown: &str) -> Vec<MarkdownBlock> {
    let mut blocks = Vec::new();
    for raw_line in markdown.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if is_bare_link(line) {
            blocks.push(MarkdownBlock {
                kind: MarkdownBlockKind::Link,
                text: line.into(),
                url: line.into(),
            });
            continue;
        }
        if let Some(heading) = line
            .strip_prefix("### ")
            .or_else(|| line.strip_prefix("## "))
            .or_else(|| line.strip_prefix("# "))
        {
            blocks.push(MarkdownBlock {
                kind: MarkdownBlockKind::Heading,
                text: strip_inline_emphasis(&strip_inline_links(heading)).into(),
                url: "".into(),
            });
            continue;
        }
        if let Some(bullet) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            blocks.push(MarkdownBlock {
                kind: MarkdownBlockKind::Bullet,
                text: strip_inline_emphasis(&strip_inline_links(bullet)).into(),
                url: "".into(),
            });
            continue;
        }
        blocks.push(MarkdownBlock {
            kind: MarkdownBlockKind::Paragraph,
            text: strip_inline_emphasis(&strip_inline_links(line)).into(),
            url: "".into(),
        });
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(blocks: &[MarkdownBlock]) -> Vec<MarkdownBlockKind> {
        blocks.iter().map(|b| b.kind).collect()
    }

    #[test]
    fn parses_headings_bullets_and_paragraphs() {
        let blocks = render_blocks("## Fixed\n\n- One thing\n- Another thing\n\nA closing note.");
        assert_eq!(
            kinds(&blocks),
            vec![
                MarkdownBlockKind::Heading,
                MarkdownBlockKind::Bullet,
                MarkdownBlockKind::Bullet,
                MarkdownBlockKind::Paragraph
            ]
        );
        assert_eq!(blocks[0].text, "Fixed");
        assert_eq!(blocks[1].text, "One thing");
    }

    #[test]
    fn recognizes_a_whole_line_link() {
        let blocks = render_blocks("https://github.com/example/repo/releases/tag/v1.0.0");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, MarkdownBlockKind::Link);
        assert_eq!(
            blocks[0].url,
            "https://github.com/example/repo/releases/tag/v1.0.0"
        );
    }

    #[test]
    fn strips_bold_markers() {
        let blocks = render_blocks("This is **bold** text.");
        assert_eq!(blocks[0].text, "This is bold text.");
    }

    #[test]
    fn drops_inline_link_url_keeps_label() {
        let blocks = render_blocks("See [the release notes](https://example.com) for details.");
        assert_eq!(blocks[0].text, "See the release notes for details.");
    }
}
