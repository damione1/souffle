/**
 * Two locales, one set of templates: every page template renders once per
 * entry in `locales`, and `_data/en.json` / `_data/fr.json` supply the copy.
 * Output is plain static files — no server, nothing to configure on
 * Cloudflare Pages beyond "run npm run build, publish _site".
 */
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const srcDir = join(dirname(fileURLToPath(import.meta.url)), "src");

export default function (eleventyConfig) {
  eleventyConfig.addPassthroughCopy({ "src/css": "css" });
  eleventyConfig.addPassthroughCopy({ "src/js": "js" });
  eleventyConfig.addPassthroughCopy({ "src/fonts": "fonts" });
  eleventyConfig.addPassthroughCopy({ "src/souffle-logo.svg": "souffle-logo.svg" });
  eleventyConfig.addPassthroughCopy({ "src/favicon.ico": "favicon.ico" });
  eleventyConfig.addPassthroughCopy({ "src/apple-touch-icon.png": "apple-touch-icon.png" });
  eleventyConfig.addPassthroughCopy({ "src/icon-192.png": "icon-192.png" });
  eleventyConfig.addPassthroughCopy({ "src/icon-512.png": "icon-512.png" });
  eleventyConfig.addPassthroughCopy({ "src/og-en.png": "og-en.png" });
  eleventyConfig.addPassthroughCopy({ "src/og-fr.png": "og-fr.png" });
  eleventyConfig.addPassthroughCopy({ "src/_headers": "_headers" });
  eleventyConfig.addPassthroughCopy({ "src/robots.txt": "robots.txt" });

  // CSS and JS keep stable filenames, so a deploy would otherwise serve fresh
  // HTML against whatever stylesheet the visitor still had cached. Stamping a
  // content hash onto the URL makes each build a distinct cache entry.
  eleventyConfig.addFilter("bust", (url) => {
    const digest = createHash("sha256").update(readFileSync(join(srcDir, url))).digest("hex");
    return `${url}?v=${digest.slice(0, 8)}`;
  });

  // `t.some.key` inside a template, resolved against the page's locale bundle.
  eleventyConfig.addFilter("get", (obj, path) =>
    String(path).split(".").reduce((acc, k) => (acc == null ? acc : acc[k]), obj),
  );

  // Serializes a plain object to a JSON-LD-safe string (escapes "<" so a
  // stray "</script>" inside copy can't break out of the script tag).
  eleventyConfig.addFilter("jsonld", (obj) =>
    JSON.stringify(obj).replace(/</g, "\\u003c"),
  );

  return {
    dir: { input: "src", output: "_site", includes: "_includes", data: "_data" },
    markdownTemplateEngine: "njk",
    htmlTemplateEngine: "njk",
  };
}
