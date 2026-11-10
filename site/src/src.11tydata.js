/**
 * Per-page computed data for everything under src/.
 *
 * `t` is the locale bundle the templates read (`t.hero.title`, `t.mock.…`).
 * It has to be computed in JS rather than in front matter: a front-matter
 * `{{ ... }}` would hand the template a stringified object, not the object.
 */
import locales from "./_data/locales.js";

const localeFor = (data) => data.docPage?.loc ?? data.loc ?? locales[0];
const bundleFor = (data) => data[localeFor(data).code];

export default {
  eleventyComputed: {
    locale: localeFor,
    t: bundleFor,
    guide: (data) => data.docPage?.guide,
    translationPath: (data) => data.isContact ? "contact/" : data.docPage ? `docs/${data.docPage.guide.slug}/` : data.isDocs ? "docs/" : "",
    pageTitle: (data) => {
      const t = bundleFor(data);
      if (!t) return "";
      if (data.isContact) return data.contact[data.loc.code].title;
      if (data.isNotFound) return "Page not found / Page introuvable — Soufflé";
      if (data.docPage) {
        const title = data.docPage.guide[localeFor(data).code].title;
        return title.includes(data.site.name) ? title : `${title} | ${data.site.name}`;
      }
      return data.isDocs ? t.meta.docsTitle : t.meta.title;
    },
    pageDescription: (data) => {
      const t = bundleFor(data);
      if (!t) return "";
      if (data.isContact) return data.contact[data.loc.code].intro;
      if (data.isNotFound) return "Find Soufflé's homepage and documentation / Retrouvez l'accueil et la documentation de Soufflé.";
      if (data.docPage) return data.docPage.guide[localeFor(data).code].description;
      return data.isDocs ? t.meta.docsDescription : t.meta.description;
    },
  },
};
