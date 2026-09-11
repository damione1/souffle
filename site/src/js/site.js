/* Copy-to-clipboard on the ruled command lines. */
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
})();

/* Flush-set billing.
   A bill sets its headline to the sheet: each line is measured at a
   reference size and scaled so it fills the measure exactly. Lines were
   rebroken per locale to comparable lengths, so the block squares off
   without any one line towering over its neighbours. CSS carries a clamp
   for the moment before this runs and for the case where it never does. */
(function () {
  "use strict";
  var blocks = Array.prototype.slice.call(document.querySelectorAll("[data-fit]"));
  if (!blocks.length) return;

  var REF = 100;
  /* High enough that the measure is what stops a line, not the cap. */
  var MAX = 220;
  var MIN = 30;

  var fit = function () {
    blocks.forEach(function (block) {
      var width = block.clientWidth;
      if (!width) return;
      Array.prototype.forEach.call(block.querySelectorAll(".hl"), function (line) {
        line.style.fontSize = REF + "px";
        var natural = line.getBoundingClientRect().width;
        if (!natural) return;
        var size = Math.min(MAX, Math.max(MIN, (width / natural) * REF));
        line.style.fontSize = size.toFixed(3) + "px";

        /* One correction pass: the first estimate is taken against a
           rendering at the reference size, and hinting at the final size
           moves the real width a little. Without this the line overhangs
           the margin every rule below it stops at. */
        var actual = line.getBoundingClientRect().width;
        if (actual > 0) {
          size = Math.min(MAX, Math.max(MIN, size * (width / actual)));
          line.style.fontSize = size.toFixed(3) + "px";
        }
      });
    });
  };

  var queued = false;
  var onResize = function () {
    if (queued) return;
    queued = true;
    requestAnimationFrame(function () { queued = false; fit(); });
  };

  fit();
  window.addEventListener("resize", onResize);
  /* The face swaps in after first paint; a measurement taken against the
     fallback would set every line to the wrong width. */
  if (document.fonts && document.fonts.ready) document.fonts.ready.then(fit);
})();

/* The billing size-step: the page's one authored moment.
   Nothing here fades in or slides up — every element is visible from the
   start and this only changes rank. The act you are reading takes top
   billing (its heading steps up a size) and inverts its name in the run
   across the top; the acts behind you take a struck rule, the way a bill
   marks a set that has already played.

   Under reduced motion the size step is disabled in CSS and rank survives
   as the inversion and the strike alone. */
(function () {
  "use strict";
  var acts = Array.prototype.slice.call(document.querySelectorAll("[data-act-watch]"));
  if (!acts.length) return;

  var runLinks = {};
  document.querySelectorAll(".run-acts a[data-act]").forEach(function (a) {
    runLinks[a.getAttribute("data-act")] = a;
  });

  /* Document order of the groups, so acts behind the reader can be struck. */
  var groupOrder = [];
  acts.forEach(function (el) {
    var g = el.getAttribute("data-act-watch");
    if (g && groupOrder.indexOf(g) < 0) groupOrder.push(g);
  });

  /* Reserve each name's widest state.
     The step changes the size of a name in the bill, which would otherwise
     shove its neighbours sideways every time rank changed. Each name is
     measured at its largest state once and pinned to that width, so the row
     holds still while the type inside it steps. */
  var reserve = function () {
    Object.keys(runLinks).forEach(function (key) {
      var a = runLinks[key];
      a.style.minWidth = "";
      var had = a.className;
      /* The size is transitioned, so a width read straight after adding the
         class returns the size it is animating away from. */
      a.style.transition = "none";
      a.classList.remove("is-playing", "is-demoted");
      a.classList.add("is-playing");
      var widest = a.getBoundingClientRect().width;
      a.className = had;
      a.style.minWidth = Math.ceil(widest) + 1 + "px";
      void a.offsetWidth;
      a.style.transition = "";
    });
  };

  /* Rank is read off one line across the viewport rather than off an
     intersection band: the act in play is simply the last one whose top has
     passed that line. A band leaves gaps between acts where nothing
     qualifies, and the bill would blink empty on the way down. */
  var current = function () {
    var line = window.innerHeight * 0.35;
    var found = null;
    acts.forEach(function (el) {
      if (el.getBoundingClientRect().top <= line) found = el;
    });
    return found;
  };

  var last;
  var paint = function () {
    var playing = current();
    if (playing === last) return;
    last = playing;
    var playingAt = playing ? acts.indexOf(playing) : -1;

    acts.forEach(function (el, i) {
      el.classList.toggle("is-played", playingAt >= 0 && i < playingAt);
    });

    var group = playing ? playing.getAttribute("data-act-watch") : null;
    var groupAt = group ? groupOrder.indexOf(group) : -1;
    groupOrder.forEach(function (key, i) {
      var a = runLinks[key];
      if (!a) return;
      a.classList.toggle("is-playing", key === group);
      a.classList.toggle("is-demoted", groupAt >= 0 && key !== group);
      a.classList.toggle("is-played", groupAt >= 0 && i < groupAt);
    });
  };

  var queued = false;
  var onScroll = function () {
    if (queued) return;
    queued = true;
    requestAnimationFrame(function () { queued = false; paint(); });
  };

  var onResize = function () {
    reserve();
    last = undefined;
    paint();
  };

  window.addEventListener("scroll", onScroll, { passive: true });
  window.addEventListener("resize", onResize);
  reserve();
  paint();
  /* The face swaps in after first paint and changes every measured width. */
  if (document.fonts && document.fonts.ready) document.fonts.ready.then(onResize);
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
  }, { rootMargin: "-24px 0px -70% 0px" });
  sections.forEach(function (s) { io.observe(s); });
})();

/* Narrow screens: the bill of acts folds into the top line, opened by the
   toggle. CSS hides the panel above 780px, so a class left behind by a
   resize costs nothing. */
(function () {
  "use strict";
  var toggle = document.querySelector(".run-toggle");
  var run = document.getElementById("margin-run");
  if (!toggle || !run) return;

  var open = toggle.querySelector(".run-toggle-open");
  var shut = toggle.querySelector(".run-toggle-close");

  var set = function (isOpen) {
    run.classList.toggle("is-open", isOpen);
    toggle.setAttribute("aria-expanded", isOpen ? "true" : "false");
    if (open) open.hidden = isOpen;
    if (shut) shut.hidden = !isOpen;
  };

  toggle.addEventListener("click", function () {
    set(!run.classList.contains("is-open"));
  });

  run.addEventListener("click", function (e) {
    if (e.target.closest(".run-acts a, .run-langs a")) set(false);
  });

  document.addEventListener("keydown", function (e) {
    if (e.key === "Escape") set(false);
  });

  document.addEventListener("click", function (e) {
    if (!e.target.closest("#margin-run")) set(false);
  });

  window.matchMedia("(min-width: 781px)").addEventListener("change", function () { set(false); });
})();
