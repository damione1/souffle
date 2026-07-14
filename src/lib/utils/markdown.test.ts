import { describe, it, expect } from "vitest";
import { renderReleaseNotesMarkdown } from "./markdown";

describe("renderReleaseNotesMarkdown", () => {
  it("renders headings up to level 4", () => {
    expect(renderReleaseNotesMarkdown("# Title")).toBe("<h1>Title</h1>");
    expect(renderReleaseNotesMarkdown("## Fixes")).toBe("<h2>Fixes</h2>");
    expect(renderReleaseNotesMarkdown("#### Deep")).toBe("<h4>Deep</h4>");
  });

  it("does not treat deeper hashes as headings", () => {
    expect(renderReleaseNotesMarkdown("##### Nope")).toBe("<p>##### Nope</p>");
  });

  it("renders unordered lists from - and * bullets", () => {
    expect(renderReleaseNotesMarkdown("- one\n- two")).toBe("<ul><li>one</li><li>two</li></ul>");
    expect(renderReleaseNotesMarkdown("* one\n* two")).toBe("<ul><li>one</li><li>two</li></ul>");
  });

  it("renders ordered lists", () => {
    expect(renderReleaseNotesMarkdown("1. first\n2. second")).toBe(
      "<ol><li>first</li><li>second</li></ol>",
    );
  });

  it("renders bold, italic, and inline code", () => {
    expect(renderReleaseNotesMarkdown("**bold** and _italic_ and `code`")).toBe(
      "<p><strong>bold</strong> and <em>italic</em> and <code>code</code></p>",
    );
  });

  it("groups consecutive plain lines into one paragraph with line breaks", () => {
    expect(renderReleaseNotesMarkdown("line one\nline two\n\nnext para")).toBe(
      "<p>line one<br>line two</p>\n<p>next para</p>",
    );
  });

  it("linkifies markdown links to github.com", () => {
    expect(renderReleaseNotesMarkdown("[PR #50](https://github.com/damione1/souffle/pull/50)")).toBe(
      '<p><a href="https://github.com/damione1/souffle/pull/50">PR #50</a></p>',
    );
  });

  it("linkifies bare github.com URLs", () => {
    expect(renderReleaseNotesMarkdown("See https://github.com/damione1/souffle/pull/50")).toBe(
      '<p>See <a href="https://github.com/damione1/souffle/pull/50">https://github.com/damione1/souffle/pull/50</a></p>',
    );
  });

  it("leaves non-github and non-https destinations as plain text", () => {
    expect(renderReleaseNotesMarkdown("[x](https://evil.example.com/)")).toBe(
      "<p>[x](https://evil.example.com/)</p>",
    );
    expect(renderReleaseNotesMarkdown("[x](javascript:alert(1))")).toBe(
      "<p>[x](javascript:alert(1))</p>",
    );
    expect(renderReleaseNotesMarkdown("http://github.com/insecure")).toBe(
      "<p>http://github.com/insecure</p>",
    );
  });

  it("escapes raw HTML instead of passing it through", () => {
    expect(renderReleaseNotesMarkdown('<img src=x onerror="alert(1)">')).toBe(
      "<p>&lt;img src=x onerror=&quot;alert(1)&quot;&gt;</p>",
    );
    expect(renderReleaseNotesMarkdown("<script>alert(1)</script>")).toBe(
      "<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>",
    );
  });

  it("escapes HTML inside list items and headings", () => {
    expect(renderReleaseNotesMarkdown("- <b>hi</b>")).toBe("<ul><li>&lt;b&gt;hi&lt;/b&gt;</li></ul>");
    expect(renderReleaseNotesMarkdown("## <script>x</script>")).toBe(
      "<h2>&lt;script&gt;x&lt;/script&gt;</h2>",
    );
  });

  it("does not let a quote in link text break out of the href", () => {
    const html = renderReleaseNotesMarkdown('["><img src=x>](https://github.com/x)');
    expect(html).not.toContain("<img");
    expect(html).toContain("&quot;&gt;&lt;img src=x&gt;");
  });

  it("does not read underscores inside URLs or code spans as emphasis", () => {
    expect(
      renderReleaseNotesMarkdown("https://github.com/a/b/compare/v0_1...v0_2"),
    ).toContain('href="https://github.com/a/b/compare/v0_1...v0_2"');
    expect(renderReleaseNotesMarkdown("`snake_case_name`")).toBe(
      "<p><code>snake_case_name</code></p>",
    );
  });

  it("strips NUL characters from input so placeholders cannot be forged", () => {
    const nul = String.fromCharCode(0);
    const html = renderReleaseNotesMarkdown(`before ${nul}0${nul} after **b**`);
    expect(html).toBe("<p>before 0 after <strong>b</strong></p>");
  });

  it("handles CRLF line endings", () => {
    expect(renderReleaseNotesMarkdown("# Title\r\n\r\n- item")).toBe(
      "<h1>Title</h1>\n<ul><li>item</li></ul>",
    );
  });

  it("renders the current release template shape", () => {
    const notes = [
      "Download the .dmg below for Apple Silicon Macs.",
      "",
      "## What's Changed",
      "* Fix a bug by @damione1 in https://github.com/damione1/souffle/pull/50",
      "",
      "**Full Changelog**: https://github.com/damione1/souffle/compare/v0.5.6...v0.5.7",
    ].join("\n");
    const html = renderReleaseNotesMarkdown(notes);
    expect(html).toContain("<h2>What&#39;s Changed</h2>");
    expect(html).toContain('<a href="https://github.com/damione1/souffle/pull/50">');
    expect(html).toContain("<strong>Full Changelog</strong>");
  });

  it("returns an empty string for empty input", () => {
    expect(renderReleaseNotesMarkdown("")).toBe("");
    expect(renderReleaseNotesMarkdown("\n\n")).toBe("");
  });
});
