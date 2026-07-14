/**
 * Minimal, dependency-free Markdown-to-HTML renderer for GitHub release
 * notes. Release note bodies come from the GitHub API (hand-written by a
 * maintainer or auto-generated from merged PR titles), so they're treated as
 * untrusted input for rendering purposes: every text run is HTML-escaped
 * before any markup is added, and only a fixed, closed set of tags is ever
 * emitted. There is no raw-HTML passthrough, so `{@html ...}` on the result
 * is safe without an external sanitizer.
 *
 * Supported subset: headings (`#`..`####`), bold (`**text**`), italic
 * (`_text_`), inline code (`` `code` ``), links (`[text](https://...)`),
 * bare `https://` autolinks, and unordered/ordered lists. Anything else
 * renders as plain escaped text. Only `https://github.com/` links are
 * linkified, mirroring the `open_release_page` command's validation so every
 * rendered link is guaranteed to open; other URLs and schemes (javascript:,
 * data:, mailto:, http:) are left as literal text.
 */

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

const BULLET_RE = /^[-*]\s+(.*)$/;
const ORDERED_RE = /^\d+\.\s+(.*)$/;
const HEADING_RE = /^(#{1,4})\s+(.*)$/;

// U+0000 never appears in escapeHtml output and is stripped from the source
// markdown up front (see renderReleaseNotesMarkdown), so it's a
// collision-free delimiter for stashed inline HTML below.
// Built at runtime so this source file stays pure ASCII.
const PLACEHOLDER_BOUND = String.fromCharCode(0);
const PLACEHOLDER_RE = new RegExp(`${PLACEHOLDER_BOUND}(\\d+)${PLACEHOLDER_BOUND}`, "g");

/**
 * Renders inline markup (links, bold, italic, code) on already-escaped text.
 * Links and inline code are extracted into placeholders before bold/italic
 * run, so a `_`/`*` inside a URL or code span can never be misread as
 * emphasis, and bold/italic can never inject markup into a link's `href`.
 */
function renderInline(escaped: string): string {
  const placeholders: string[] = [];
  const stash = (html: string): string => {
    placeholders.push(html);
    return `${PLACEHOLDER_BOUND}${placeholders.length - 1}${PLACEHOLDER_BOUND}`;
  };

  let out = escaped;

  // Markdown links: only https://github.com/ destinations are linkified.
  out = out.replace(
    /\[([^[\]]+)\]\((https:\/\/github\.com\/[^\s)]+)\)/g,
    (_m, text: string, url: string) => stash(`<a href="${url}">${text}</a>`),
  );

  // Bare github.com URLs not already consumed by a markdown link above.
  out = out.replace(/https:\/\/github\.com\/[^\s<]+/g, (url) => stash(`<a href="${url}">${url}</a>`));

  // Inline code.
  out = out.replace(/`([^`]+)`/g, (_m, code: string) => stash(`<code>${code}</code>`));

  // Bold, then italic (order matters so `**_x_**` and `_**x**_` both work).
  out = out.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  out = out.replace(/_([^_]+)_/g, "<em>$1</em>");

  out = out.replace(PLACEHOLDER_RE, (_m, i: string) => placeholders[Number(i)]);

  return out;
}

/** Renders a release-notes Markdown string into a safe HTML string. */
export function renderReleaseNotesMarkdown(markdown: string): string {
  const lines = markdown
    .split(PLACEHOLDER_BOUND)
    .join("")
    .replace(/\r\n/g, "\n")
    .split("\n");
  const html: string[] = [];
  let i = 0;

  while (i < lines.length) {
    const line = lines[i];

    if (line.trim() === "") {
      i++;
      continue;
    }

    const heading = line.match(HEADING_RE);
    if (heading) {
      const level = heading[1].length;
      html.push(`<h${level}>${renderInline(escapeHtml(heading[2].trim()))}</h${level}>`);
      i++;
      continue;
    }

    if (BULLET_RE.test(line)) {
      const items: string[] = [];
      while (i < lines.length && BULLET_RE.test(lines[i])) {
        items.push(lines[i].replace(BULLET_RE, "$1"));
        i++;
      }
      html.push(`<ul>${items.map((it) => `<li>${renderInline(escapeHtml(it))}</li>`).join("")}</ul>`);
      continue;
    }

    if (ORDERED_RE.test(line)) {
      const items: string[] = [];
      while (i < lines.length && ORDERED_RE.test(lines[i])) {
        items.push(lines[i].replace(ORDERED_RE, "$1"));
        i++;
      }
      html.push(`<ol>${items.map((it) => `<li>${renderInline(escapeHtml(it))}</li>`).join("")}</ol>`);
      continue;
    }

    // Paragraph: consecutive plain lines, soft-wrapped with <br>.
    const paragraphLines: string[] = [];
    while (
      i < lines.length &&
      lines[i].trim() !== "" &&
      !HEADING_RE.test(lines[i]) &&
      !BULLET_RE.test(lines[i]) &&
      !ORDERED_RE.test(lines[i])
    ) {
      paragraphLines.push(lines[i]);
      i++;
    }
    html.push(`<p>${paragraphLines.map((l) => renderInline(escapeHtml(l))).join("<br>")}</p>`);
  }

  return html.join("\n");
}
