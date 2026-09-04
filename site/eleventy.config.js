/**
 * Two locales, one set of templates: every page template renders once per
 * entry in `locales`, and `_data/en.json` / `_data/fr.json` supply the copy.
 * Output is plain static files — no server, nothing to configure on
 * Cloudflare Pages beyond "run npm run build, publish _site".
 */
export default function (eleventyConfig) {
  eleventyConfig.addPassthroughCopy({ "src/css": "css" });
  eleventyConfig.addPassthroughCopy({ "src/js": "js" });
  eleventyConfig.addPassthroughCopy({ "src/fonts": "fonts" });
  eleventyConfig.addPassthroughCopy({ "src/souffle-logo.svg": "souffle-logo.svg" });
  eleventyConfig.addPassthroughCopy({ "src/_headers": "_headers" });

  // `t.some.key` inside a template, resolved against the page's locale bundle.
  eleventyConfig.addFilter("get", (obj, path) =>
    String(path).split(".").reduce((acc, k) => (acc == null ? acc : acc[k]), obj),
  );

  return {
    dir: { input: "src", output: "_site", includes: "_includes", data: "_data" },
    markdownTemplateEngine: "njk",
    htmlTemplateEngine: "njk",
  };
}
