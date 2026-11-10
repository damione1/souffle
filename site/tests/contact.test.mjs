// SPDX-License-Identifier: GPL-3.0-or-later
import test from 'node:test';
import assert from 'node:assert/strict';
import { contact } from '../cloudflare/contact-worker.mjs';
import worker from '../cloudflare/worker.js';

const fields = { name: 'Ada Lovelace', email: 'ada@example.test', topic: 'project', message: 'I am building a native Rust application.', locale: 'fr', website: '', 'cf-turnstile-response': 'test-token' };
function fixture(overrides = {}) {
  const sent = [], verified = [], limited = [];
  const env = {
    TURNSTILE_SITE_KEY: 'public-site-key', TURNSTILE_SECRET_KEY: 'private-turnstile-key',
    CONTACT_TO: 'private-inbox@example.test', CONTACT_FROM: 'notifications@example.test',
    CONTACT_EMAIL: { send: async message => { sent.push(message); } },
    CONTACT_RATE_LIMITER: { limit: async key => { limited.push(key); return { success: true }; } },
    ...overrides,
  };
  const fetcher = async (url, init) => {
    verified.push({ url, init });
    return Response.json({ success: true, hostname: 'usesouffle.app', action: 'contact' });
  };
  return { env, sent, verified, limited, fetcher };
}
function request(changes = {}, options = {}) {
  return new Request('https://usesouffle.app/api/contact', {
    method: 'POST', headers: { Origin: 'https://usesouffle.app', 'CF-Connecting-IP': '192.0.2.1', ...options.headers },
    body: new URLSearchParams({ ...fields, ...changes }),
  });
}

test('valid contact validates Turnstile, sends a plain-text email to the secret recipient and exposes no private data', async () => {
  const f = fixture(), response = await contact(request(), f.env, f.fetcher);
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { code: 'sent' });
  assert.equal(f.verified.length, 1);
  const verification = f.verified[0];
  assert.equal(verification.url, 'https://challenges.cloudflare.com/turnstile/v0/siteverify');
  assert.equal(verification.init.body.get('secret'), f.env.TURNSTILE_SECRET_KEY);
  assert.equal(verification.init.body.get('remoteip'), '192.0.2.1');
  assert.equal(f.sent.length, 1);
  assert.equal(f.sent[0].to, f.env.CONTACT_TO);
  assert.equal(f.sent[0].replyTo, fields.email);
  assert.equal(f.sent[0].from.email, f.env.CONTACT_FROM);
  assert.equal(f.sent[0].subject, '[Soufflé] Project or collaboration');
  assert.match(f.sent[0].text, /Language: fr\n\nI am building/);
  assert.equal(f.sent[0].html, undefined);
  assert.equal(response.headers.get('Cache-Control'), 'no-store');
  assert.equal(response.headers.get('X-Robots-Tag'), 'noindex');
});

test('public configuration exposes only the sitekey, never secrets or addresses', async () => {
  const f = fixture();
  const response = await worker.fetch(new Request('https://usesouffle.app/api/contact/config'), f.env);
  assert.deepEqual(await response.json(), { code: 'ready', sitekey: 'public-site-key' });
  for (const key of ['TURNSTILE_SECRET_KEY', 'CONTACT_TO', 'CONTACT_FROM', 'CONTACT_EMAIL', 'CONTACT_RATE_LIMITER']) {
    const result = await contact(new Request('https://usesouffle.app/api/contact/config'), { ...f.env, [key]: undefined });
    assert.equal(result.status, 503, key);
    assert.deepEqual(await result.json(), { code: 'unavailable' });
  }
});

test('method, origin, format and alternate-host guards run before email or validation', async () => {
  const f = fixture();
  for (const [req, status] of [
    [new Request('https://usesouffle.app/api/contact'), 405],
    [new Request('https://usesouffle.app/api/contact/config', { method: 'POST' }), 405],
    [request({}, { headers: { Origin: 'https://evil.example' } }), 403],
    [request({}, { headers: { Origin: '' } }), 403],
    [request({}, { headers: { 'Sec-Fetch-Site': 'cross-site' } }), 403],
    [request({}, { headers: { 'Content-Type': 'application/json' } }), 415],
    [new Request('https://preview.workers.dev/api/contact', request()), 403],
    [new Request('https://www.usesouffle.app/api/contact', request()), 403],
  ]) assert.equal((await contact(req, f.env, f.fetcher)).status, status);
  assert.equal(f.sent.length, 0); assert.equal(f.verified.length, 0);
});

test('invalid fields and header injection never call Turnstile or send mail', async () => {
  const f = fixture();
  for (const changes of [
    { name: '' }, { name: 'x'.repeat(101) }, { name: 'Ada\r\nBcc: bad@example.test' },
    { email: 'anything' }, { email: 'a@example.test\r\nBcc: bad@example.test' },
    { topic: '__proto__' }, { topic: 'arbitrary subject' }, { message: 'too short' },
    { message: 'a'.repeat(5001) }, { message: 'message with \u0000 null' }, { locale: 'es' },
  ]) assert([400, 422].includes((await contact(request(changes), f.env, f.fetcher)).status), JSON.stringify(changes));
  const body = new URLSearchParams(fields); body.append('email', 'second@example.test');
  assert.equal((await contact(new Request('https://usesouffle.app/api/contact', { method: 'POST', headers: { Origin: 'https://usesouffle.app', 'CF-Connecting-IP': '192.0.2.1' }, body }), f.env, f.fetcher)).status, 400);
  assert.equal((await contact(request({ to: 'attacker@example.test' }), f.env, f.fetcher)).status, 400);
  assert.equal(f.sent.length, 0); assert.equal(f.verified.length, 0);
});

test('missing, oversized, expired, replayed and mismatched challenges never send mail', async () => {
  const f = fixture();
  for (const token of ['', 'x'.repeat(2049)]) assert.equal((await contact(request({ 'cf-turnstile-response': token }), f.env, f.fetcher)).status, 400);
  for (const result of [
    { success: false, 'error-codes': ['timeout-or-duplicate'] },
    { success: true, hostname: 'evil.example', action: 'contact' },
    { success: true, hostname: 'usesouffle.app', action: 'login' },
    { success: true, hostname: 'usesouffle.app' },
  ]) assert.equal((await contact(request(), f.env, async () => Response.json(result))).status, 400);
  assert.equal(f.sent.length, 0);
});

test('service errors fail closed and never reveal provider errors containing addresses', async () => {
  const f = fixture({ CONTACT_EMAIL: { send: async () => { throw Error('private-inbox@example.test rejected'); } } });
  for (const fetcher of [f.fetcher, async () => { throw Error('timeout'); }, async () => new Response('failure', { status: 502 }), async () => new Response('not json')]) {
    const response = await contact(request(), f.env, fetcher);
    assert.equal(response.status, 503); assert.deepEqual(await response.json(), { code: 'unavailable' });
  }
});

test('rate limits and honeypot prevent upstream calls', async () => {
  const f = fixture({ CONTACT_RATE_LIMITER: { limit: async () => ({ success: false }) } });
  const response = await contact(request(), f.env, f.fetcher);
  assert.equal(response.status, 429); assert.equal(response.headers.get('Retry-After'), '60');
  assert.equal(f.verified.length, 0); assert.equal(f.sent.length, 0);
  const bot = fixture();
  assert.equal((await contact(request({ website: 'spam.example' }), bot.env, bot.fetcher)).status, 200);
  assert.equal(bot.verified.length, 0); assert.equal(bot.sent.length, 0);
});

test('body limit applies to streamed bytes, while a full Unicode message is accepted', async () => {
  const f = fixture();
  const response = await contact(request({ message: 'x'.repeat(65_537) }), f.env, f.fetcher);
  assert.equal(response.status, 413); assert.equal(f.verified.length, 0);
  assert.equal((await contact(request({ message: '界'.repeat(5000) }), f.env, f.fetcher)).status, 200);
  const malformed = new Request('https://usesouffle.app/api/contact', { method: 'POST', headers: { Origin: 'https://usesouffle.app', 'CF-Connecting-IP': '192.0.2.1', 'Content-Type': 'application/x-www-form-urlencoded' }, body: new Uint8Array([255]) });
  assert.equal((await contact(malformed, f.env, f.fetcher)).status, 400);
});

test('unknown API paths are not mail endpoints', async () => {
  const response = await worker.fetch(new Request('https://usesouffle.app/api/contact/unknown'), {});
  assert.equal(response.status, 404);
});
