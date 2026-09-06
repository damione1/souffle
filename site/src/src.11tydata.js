/**
 * Per-page computed data for everything under src/.
 *
 * `t` is the locale bundle the templates read (`t.hero.title`, `t.mock.…`).
 * It has to be computed in JS rather than in front matter: a front-matter
 * `{{ ... }}` would hand the template a stringified object, not the object.
 */
export default {
  eleventyComputed: {
    t: (data) => (data.loc && data.loc.code === "fr" ? data.fr : data.en),
    pageTitle: (data) => {
      const t = data.loc && data.loc.code === "fr" ? data.fr : data.en;
      if (!t) return "";
      return data.isDocs ? t.meta.docsTitle : t.meta.title;
    },
  },
};
