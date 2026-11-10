import { contact } from './contact-worker.mjs';
export default {
  fetch(request, env) {
    const path = new URL(request.url).pathname;
    if (path === '/api/contact' || path === '/api/contact/config') return contact(request, env);
    return new Response('Not found', { status: 404 });
  },
};
