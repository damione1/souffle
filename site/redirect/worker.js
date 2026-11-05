// This Worker is routed only on www. The Pages site serves the canonical host.
export default {
  fetch(request) {
    const target = new URL(request.url);
    target.protocol = "https:";
    target.host = "usesouffle.app";
    return Response.redirect(target.href, 301);
  },
};
