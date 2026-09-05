/* Page chrome: copy-to-clipboard chips and reveal-on-scroll. */
(function () {
  "use strict";

  document.querySelectorAll("[data-copy]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      var text = btn.getAttribute("data-copy");
      var done = function () {
        btn.classList.add("is-copied");
        setTimeout(function () { btn.classList.remove("is-copied"); }, 1400);
      };
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(text).then(done, done);
      } else {
        var ta = document.createElement("textarea");
        ta.value = text; ta.setAttribute("readonly", "");
        ta.style.position = "absolute"; ta.style.left = "-9999px";
        document.body.appendChild(ta); ta.select();
        try { document.execCommand("copy"); } catch (e) { /* nothing to do */ }
        document.body.removeChild(ta); done();
      }
    });
  });

  var reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  var items = document.querySelectorAll(".reveal");
  if (reduced || !("IntersectionObserver" in window)) {
    items.forEach(function (el) { el.classList.add("is-in"); });
    return;
  }
  var io = new IntersectionObserver(function (entries) {
    entries.forEach(function (e) {
      if (!e.isIntersecting) return;
      e.target.classList.add("is-in");
      io.unobserve(e.target);
    });
  }, { rootMargin: "0px 0px -12% 0px", threshold: 0.08 });
  items.forEach(function (el) { io.observe(el); });
})();

/* Docs sidebar: highlight the section you are reading. */
(function () {
  "use strict";
  var links = Array.prototype.slice.call(document.querySelectorAll(".docs-group a"));
  if (!links.length || !("IntersectionObserver" in window)) return;
  var byId = {};
  links.forEach(function (a) { byId[a.getAttribute("href").slice(1)] = a; });
  var sections = Array.prototype.slice.call(document.querySelectorAll(".docs-section"));
  var visible = [];
  var io = new IntersectionObserver(function (entries) {
    entries.forEach(function (e) {
      var id = e.target.id;
      var at = visible.indexOf(id);
      if (e.isIntersecting && at < 0) visible.push(id);
      if (!e.isIntersecting && at >= 0) visible.splice(at, 1);
    });
    var order = sections.map(function (s) { return s.id; });
    var current = order.filter(function (id) { return visible.indexOf(id) >= 0; })[0];
    links.forEach(function (a) { a.classList.remove("is-active"); });
    if (current && byId[current]) byId[current].classList.add("is-active");
  }, { rootMargin: "-96px 0px -70% 0px" });
  sections.forEach(function (s) { io.observe(s); });
})();

/* Mobile nav: the links live in a panel under the bar, opened by the
   hamburger. The panel is CSS-hidden above 860px, so the desktop nav is
   unaffected and the class left behind by a resize costs nothing. */
(function () {
  "use strict";
  var toggle = document.querySelector(".nav-toggle");
  var panel = document.getElementById("nav-menu");
  if (!toggle || !panel) return;

  var close = function () {
    panel.classList.remove("is-open");
    toggle.setAttribute("aria-expanded", "false");
  };

  toggle.addEventListener("click", function () {
    var open = panel.classList.toggle("is-open");
    toggle.setAttribute("aria-expanded", open ? "true" : "false");
  });

  panel.addEventListener("click", function (e) {
    if (e.target.closest("a")) close();
  });

  document.addEventListener("keydown", function (e) {
    if (e.key === "Escape") close();
  });

  document.addEventListener("click", function (e) {
    if (!e.target.closest(".nav")) close();
  });

  window.matchMedia("(min-width: 861px)").addEventListener("change", close);
})();
