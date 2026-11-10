// SPDX-License-Identifier: GPL-3.0-or-later
const form = document.querySelector('[data-contact-form]');
if (form) {
  const copy = JSON.parse(document.getElementById('contact-copy').textContent);
  const status = form.querySelector('[data-contact-status]');
  const submit = form.querySelector('[type=submit]');
  const challenge = form.querySelector('[data-contact-challenge]');
  let widget, token = '', pending = false;
  function announce(message) { status.textContent = message; }
  function ready() { submit.disabled = pending || !token; }
  function resetChallenge() { token = ''; ready(); if (widget !== undefined) window.turnstile.reset(widget); }
  function clearErrors() {
    for (const name of Object.keys(copy.fields)) {
      form.elements.namedItem(name).removeAttribute('aria-invalid');
      document.getElementById(`contact-${name}-error`).textContent = '';
    }
  }
  form.addEventListener('input', event => {
    const name = event.target.name;
    if (Object.hasOwn(copy.fields, name)) {
      event.target.removeAttribute('aria-invalid');
      document.getElementById(`contact-${name}-error`).textContent = '';
    }
  });
  form.addEventListener('submit', async event => {
    event.preventDefault();
    if (pending || !form.reportValidity()) return;
    if (!token) { announce(copy.challenge); return; }
    pending = true; ready(); clearErrors(); announce(copy.sending); submit.textContent = copy.sending;
    const body = new URLSearchParams(new FormData(form));
    body.set('cf-turnstile-response', token);
    try {
      const response = await fetch('/api/contact', { method: 'POST', body, signal: AbortSignal.timeout(20_000) });
      const result = await response.json();
      if (response.ok && result.code === 'sent') { form.reset(); announce(copy.sent); }
      else {
        const messages = { challenge: copy.challenge, rate: copy.rate, invalid: copy.invalid, 'too-large': copy.tooLarge };
        announce(messages[result.code] || copy.unavailable);
        const fields = Array.isArray(result.fields) ? result.fields.filter(name => Object.hasOwn(copy.fields, name)) : [];
        for (const name of fields) {
          form.elements.namedItem(name).setAttribute('aria-invalid', 'true');
          document.getElementById(`contact-${name}-error`).textContent = copy.fields[name];
        }
        if (fields.length) form.elements.namedItem(fields[0]).focus();
      }
    } catch { announce(copy.unavailable); }
    finally { pending = false; submit.textContent = copy.submit; resetChallenge(); }
  });

  async function initialize() {
    announce(copy.loading);
    try {
      const response = await fetch('/api/contact/config', { cache: 'no-store', signal: AbortSignal.timeout(10_000) });
      if (!response.ok) throw Error('unavailable');
      const config = await response.json();
      if (!config.sitekey) throw Error('unavailable');
      await new Promise((resolve, reject) => {
        const script = document.createElement('script');
        const timeout = setTimeout(() => reject(Error('timeout')), 15_000);
        script.src = 'https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit';
        script.async = true; script.onload = () => { clearTimeout(timeout); resolve(); };
        script.onerror = () => { clearTimeout(timeout); reject(Error('unavailable')); };
        document.head.append(script);
      });
      const options = {
        sitekey: config.sitekey, action: 'contact', language: document.documentElement.lang,
        callback: value => { token = value; ready(); if ([copy.loading, copy.checking, copy.challenge].includes(status.textContent)) announce(''); },
        'expired-callback': () => { token = ''; ready(); if (!pending) announce(copy.challenge); },
        'error-callback': () => { token = ''; ready(); if (!pending) announce(copy.challenge); },
      };
      let theme, size;
      function render() {
        const nextTheme = document.documentElement.dataset.theme || (document.documentElement.classList.contains('dark') ? 'dark' : 'light');
        const nextSize = challenge.clientWidth < 300 ? 'compact' : 'flexible';
        if (theme === nextTheme && size === nextSize) return;
        theme = nextTheme; size = nextSize; token = ''; ready();
        if (widget !== undefined) window.turnstile.remove(widget);
        widget = window.turnstile.render(challenge, { ...options, theme, size });
      }
      render();
      new MutationObserver(render).observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme', 'class'] });
      new ResizeObserver(render).observe(challenge);
      if (!token) announce(copy.checking);
    } catch { announce(copy.unavailable); }
  }
  initialize();
}
