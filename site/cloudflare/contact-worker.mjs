// SPDX-License-Identifier: GPL-3.0-or-later
const topics = Object.freeze({
  project: 'Project or collaboration', support: 'Help and support', feedback: 'Feedback', other: 'Other',
});
const maxBodyBytes = 65_536;
const emailPattern = /^[a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$/;
const controlCharacters = /[\u0000-\u001f\u007f]/;

function reply(status, code, extra = {}) {
  return Response.json({ code, ...extra }, { status, headers: {
    'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff',
    'X-Robots-Tag': 'noindex', ...(status === 429 ? { 'Retry-After': '60' } : {}),
  } });
}

function configured(env) {
  return Boolean(env.TURNSTILE_SITE_KEY && env.TURNSTILE_SECRET_KEY
    && validEmail(env.CONTACT_TO) && validEmail(env.CONTACT_FROM)
    && env.CONTACT_EMAIL?.send && env.CONTACT_RATE_LIMITER?.limit);
}

function validEmail(value) {
  return typeof value === 'string' && value.length <= 254 && emailPattern.test(value);
}

// Bound the stream itself; Content-Length is optional and cannot be trusted.
async function readForm(request) {
  const reader = request.body?.getReader();
  if (!reader) return new URLSearchParams();
  let size = 0;
  const chunks = [];
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > maxBodyBytes) { await reader.cancel(); return null; }
    chunks.push(value);
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
  return new URLSearchParams(new TextDecoder('utf-8', { fatal: true }).decode(bytes));
}

export async function contact(request, env, fetcher = fetch) {
  const url = new URL(request.url);
  // Preview/alias domains cannot become alternate public mail endpoints.
  if (!['usesouffle.app', 'localhost', '127.0.0.1'].includes(url.hostname)) return reply(403, 'forbidden');
  if (url.pathname === '/api/contact/config') {
    if (request.method !== 'GET') return reply(405, 'method');
    return configured(env) ? reply(200, 'ready', { sitekey: env.TURNSTILE_SITE_KEY }) : reply(503, 'unavailable');
  }
  if (request.method !== 'POST') return reply(405, 'method');
  if (request.headers.get('Origin') !== url.origin || request.headers.get('Sec-Fetch-Site') === 'cross-site') return reply(403, 'forbidden');
  if (request.headers.get('Content-Type')?.split(';')[0].trim().toLowerCase() !== 'application/x-www-form-urlencoded') return reply(415, 'format');
  if (!configured(env)) return reply(503, 'unavailable');

  try {
    const ip = request.headers.get('CF-Connecting-IP');
    if (!ip) return reply(503, 'unavailable');
    const limited = await env.CONTACT_RATE_LIMITER.limit({ key: `contact:${ip}` });
    if (!limited.success) return reply(429, 'rate');
    let form;
    try { form = await readForm(request); } catch { return reply(400, 'invalid'); }
    if (!form) return reply(413, 'too-large');
    const allowed = new Set(['name', 'email', 'topic', 'message', 'website', 'locale', 'cf-turnstile-response']);
    for (const key of form.keys()) {
      if (!allowed.has(key) || form.getAll(key).length !== 1) return reply(400, 'invalid');
    }
    // Honeypot replies look ordinary but never send mail.
    if (form.get('website')) return reply(200, 'sent');
    const name = (form.get('name') || '').trim();
    const email = (form.get('email') || '').trim();
    const topic = form.get('topic');
    const message = (form.get('message') || '').trim();
    const locale = form.get('locale');
    const fields = [];
    if (!name || name.length > 100 || controlCharacters.test(name)) fields.push('name');
    if (!validEmail(email)) fields.push('email');
    if (!Object.hasOwn(topics, topic)) fields.push('topic');
    if (message.length < 10 || message.length > 5000 || /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(message)) fields.push('message');
    if (locale !== 'en' && locale !== 'fr') return reply(400, 'invalid');
    if (fields.length) return reply(422, 'invalid', { fields });
    const token = form.get('cf-turnstile-response');
    if (!token || token.length > 2048) return reply(400, 'challenge');
    let verification;
    try {
      const result = await fetcher('https://challenges.cloudflare.com/turnstile/v0/siteverify', {
        method: 'POST', body: new URLSearchParams({ secret: env.TURNSTILE_SECRET_KEY, response: token, remoteip: ip }),
        signal: AbortSignal.timeout(10_000),
      });
      if (!result.ok) return reply(503, 'unavailable');
      verification = await result.json();
    } catch { return reply(503, 'unavailable'); }
    if (verification.success !== true || verification.hostname !== url.hostname || verification.action !== 'contact') return reply(400, 'challenge');

    // Destination and sender are Worker secrets, never browser inputs or build data.
    await env.CONTACT_EMAIL.send({
      to: env.CONTACT_TO, from: { email: env.CONTACT_FROM, name: 'Soufflé' }, replyTo: email,
      subject: `[Soufflé] ${topics[topic]}`,
      text: `Name: ${name}\nEmail: ${email}\nTopic: ${topics[topic]}\nLanguage: ${locale}\n\n${message}`,
    });
    return reply(200, 'sent');
  } catch {
    // Provider errors may contain private addresses: never expose or log them.
    return reply(503, 'unavailable');
  }
}
