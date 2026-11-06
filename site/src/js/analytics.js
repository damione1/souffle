// SPDX-License-Identifier: GPL-3.0-or-later
(() => {
  if (window.siteAnalytics) return;
  const events = new Set(['app_download', 'release_page_open', 'github_visit', 'issue_open',
    'docs_open', 'doc_read', 'install_command_copy', 'code_copy', 'web_vital']);
  const allowed = new Set(['app_version', 'download_platform', 'download_architecture', 'release_channel',
    'page_locale', 'content_type', 'content_id', 'link_placement', 'installation_method',
    'metric_name', 'metric_rating', 'metric_id', 'metric_value', 'value']);
  const vitalsSource = document.currentScript?.dataset.webVitals;
  function track(name, parameters = {}) {
    if (!events.has(name) || location.hostname !== 'usesouffle.app' || typeof window.gtag !== 'function') return;
    const route = location.pathname.replace(/^\/fr(?=\/)/, '');
    const values = {page_locale:document.documentElement.lang === 'fr' ? 'fr' : 'en',
      content_group:route.startsWith('/docs/') ? 'documentation' : route === '/404.html' ? 'not_found' : 'landing'};
    if (route.startsWith('/docs/')) values.content_id = route;
    for (const [key,value] of Object.entries(parameters)) {
      if (!allowed.has(key)) continue;
      if (typeof value === 'string' && value.length > 0 && value.length <= 100) values[key] = value;
      else if (typeof value === 'number' && Number.isFinite(value) && (value >= 0 || (name === 'web_vital' && key === 'value'))) values[key] = value;
    }
    try { if (sessionStorage.getItem('site-analytics-debug') === '1') values.debug_mode = true; } catch {}
    window.gtag('event', name, values);
  }
  window.siteAnalytics = {track};
  const placement = link => link.closest('header') || link.closest('nav') ? 'navigation'
    : link.closest('footer') ? 'footer' : link.closest('#download') ? 'download_section' : 'content';
  function click(event) {
    if ((event.type === 'auxclick' && event.button !== 1) || (event.type === 'click' && event.button > 0)) return;
    const link = event.target.closest?.('a[href]');
    if (!link) return;
    const url = new URL(link.href, location.href), parameters = {link_placement:placement(link)};
    if (link.dataset.downloadVersion && url.origin === 'https://github.com' &&
        url.pathname.startsWith('/damione1/souffle/releases/download/')) {
      track('app_download', {...parameters, app_version:link.dataset.downloadVersion,
        download_platform:link.dataset.downloadPlatform, download_architecture:link.dataset.downloadArchitecture,
        release_channel:link.dataset.releaseChannel});
    } else if (url.hostname === 'github.com' && /^\/damione1\/souffle(?:\/|$)/.test(url.pathname)) {
      const eventName = /\/releases(?:\/|$)/.test(url.pathname) ? 'release_page_open'
        : /\/issues(?:\/|$)/.test(url.pathname) ? 'issue_open' : 'github_visit';
      track(eventName, parameters);
    } else if (url.origin === location.origin && /^\/(?:fr\/)?docs\//.test(url.pathname)) {
      track('docs_open', {...parameters,content_id:url.pathname.replace(/^\/fr(?=\/)/, '')});
    }
  }
  document.addEventListener('click', click);
  document.addEventListener('auxclick', click);

  // A reading signal requires both foreground time and progress through the guide.
  if (/^\/(?:fr\/)?docs\//.test(location.pathname)) {
    const main = document.querySelector('main');
    let activeMs = 0, started = document.visibilityState === 'visible' ? performance.now() : null, reported = false;
    const check = () => {
      const now = performance.now();
      if (started !== null) activeMs += now - started;
      started = document.visibilityState === 'visible' ? now : null;
      if (!main || reported || activeMs < 30000) return;
      const box = main.getBoundingClientRect();
      if ((innerHeight - box.top) / Math.max(1,box.height) < 0.75) return;
      reported = true; clearInterval(timer); track('doc_read', {value:Math.floor(activeMs / 1000)});
    };
    const timer = setInterval(check, 1000);
    document.addEventListener('visibilitychange', check);
    window.addEventListener('scroll', check, {passive:true});
    window.addEventListener('pagehide', check);
  }
  if (vitalsSource && location.hostname === 'usesouffle.app') {
    import(vitalsSource).then(({onLCP,onINP,onCLS}) => {
      const report = metric => track('web_vital', {metric_name:metric.name,metric_rating:metric.rating,
        metric_id:metric.id,metric_value:metric.value,value:metric.delta});
      onLCP(report); onINP(report); onCLS(report);
    }).catch(() => {});
  }
})();
