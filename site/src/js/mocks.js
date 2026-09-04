/* ─────────────────────────────────────────────────────────────
   The app screens on this page are not pictures. They run the
   same behaviour the real app does:

     · the waveform is a port of Waveform.svelte's draw loop
       (48 bars, 3px wide, 2px gap, same easing and alpha curve),
       fed a synthetic speech envelope instead of AudioLevel events
     · the recording dot and the caret use src/app.css keyframes
     · the elapsed clock counts the way LiveSessionCard formats it
     · the live transcript commits a tentative tail into paragraphs,
       the way live-transcript.svelte.ts does
     · the overlay walks the PillApp states: dictating → the pill
       widens for live text → reformulating → paste

   Everything is idle until scrolled into view, and honours
   prefers-reduced-motion by rendering the finished state.
   ───────────────────────────────────────────────────────────── */
(function () {
  "use strict";

  var REDUCED = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  /* ── Waveform: Waveform.svelte, with a synthetic level ───────── */
  var BAR_COUNT = 48, BAR_WIDTH = 3, BAR_GAP = 2;

  function Wave(canvas) {
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d");
    this.bars = [];
    for (var i = 0; i < BAR_COUNT; i++) this.bars.push(0.15 + Math.random() * 0.1);
    this.level = 0;
    this.active = false;
    this.raf = 0;
    this.accent = "#e9ae55";
    this.resize();
    this.refreshAccent();
  }
  Wave.prototype.refreshAccent = function () {
    var v = getComputedStyle(this.canvas).getPropertyValue("--wave-accent").trim();
    if (v) this.accent = v;
  };
  Wave.prototype.resize = function () {
    var rect = this.canvas.getBoundingClientRect();
    if (!rect.width) return;
    var dpr = window.devicePixelRatio || 1;
    this.canvas.width = rect.width * dpr;
    this.canvas.height = rect.height * dpr;
    this.ctx.setTransform(1, 0, 0, 1, 0, 0);
    this.ctx.scale(dpr, dpr);
    this.cssW = rect.width;
    this.cssH = rect.height;
  };
  /* A speech envelope: bursts of energy with short gaps, so the bars
     move like someone talking rather than like a music visualiser. */
  Wave.prototype.envelope = function (now) {
    var phrase = (now / 2600) % 1;
    var speaking = phrase < 0.82;
    if (!speaking) return 0.06;
    var syllable = 0.5 + 0.5 * Math.sin(now / 110);
    var stress = 0.5 + 0.5 * Math.sin(now / 430 + 1.2);
    return 0.22 + syllable * 0.34 * (0.6 + stress * 0.6);
  };
  Wave.prototype.draw = function () {
    var ctx = this.ctx;
    if (!ctx || !this.cssW) { this.raf = 0; return; }
    var now = Date.now();
    var w = this.cssW, h = this.cssH;
    ctx.clearRect(0, 0, w, h);
    this.level = this.active ? this.envelope(now) : 0;

    var total = BAR_COUNT * (BAR_WIDTH + BAR_GAP) - BAR_GAP;
    var offsetX = (w - total) / 2;
    for (var i = 0; i < BAR_COUNT; i++) {
      var target;
      if (this.active) {
        var variation = Math.sin(now / 200 + i * 0.5) * 0.15;
        var spread = Math.sin(i * 0.3 + now / 300) * 0.1;
        target = Math.max(0.05, this.level + variation + spread);
        this.bars[i] += (target - this.bars[i]) * 0.3;
        this.bars[i] = Math.max(0.05, Math.min(1, this.bars[i]));
      } else {
        target = 0.12 + Math.sin(now / 800 + i * 0.3) * 0.08;
        this.bars[i] += (target - this.bars[i]) * 0.08;
      }
      var barH = this.bars[i] * (h - 4);
      var x = offsetX + i * (BAR_WIDTH + BAR_GAP);
      var y = (h - barH) / 2;
      ctx.fillStyle = this.accent;
      ctx.globalAlpha = this.active ? 0.4 + this.bars[i] * 0.6 : 0.15 + this.bars[i] * 0.2;
      ctx.beginPath();
      if (ctx.roundRect) { ctx.roundRect(x, y, BAR_WIDTH, barH, 1.5); }
      else { ctx.rect(x, y, BAR_WIDTH, barH); }
      ctx.fill();
      ctx.globalAlpha = 1;
    }
    this.raf = requestAnimationFrame(this.draw.bind(this));
  };
  Wave.prototype.start = function (active) {
    this.active = active !== false;
    this.resize();
    if (!this.raf) this.draw();
  };
  Wave.prototype.stop = function () {
    if (this.raf) cancelAnimationFrame(this.raf);
    this.raf = 0;
  };

  /* ── Elapsed clock, formatted as LiveSessionCard does ────────── */
  function parseClock(s) {
    var p = String(s).split(":");
    return p.length === 3
      ? (+p[0]) * 3600 + (+p[1]) * 60 + (+p[2])
      : (+p[0]) * 60 + (+p[1]);
  }
  function formatClock(total) {
    var m = Math.floor(total / 60), s = total % 60;
    return m + ":" + String(s).padStart(2, "0");
  }

  function Timer(el) {
    this.el = el;
    this.base = parseClock(el.getAttribute("data-start") || "0:00");
    this.seconds = this.base;
    this.id = 0;
  }
  Timer.prototype.render = function () { this.el.textContent = formatClock(this.seconds); };
  Timer.prototype.start = function () {
    var self = this;
    this.render();
    if (this.id) return;
    this.id = setInterval(function () { self.seconds += 1; self.render(); }, 1000);
  };
  Timer.prototype.stop = function () { clearInterval(this.id); this.id = 0; };
  Timer.prototype.reset = function () { this.seconds = this.base; this.render(); };

  /* ── Helpers ─────────────────────────────────────────────────── */
  function words(text) { return text.split(/(\s+)/).filter(function (w) { return w.length; }); }

  function Runner() { this.timers = []; this.stopped = false; }
  Runner.prototype.after = function (ms, fn) {
    var self = this;
    var id = setTimeout(function () { if (!self.stopped) fn(); }, ms);
    this.timers.push(id);
    return id;
  };
  Runner.prototype.every = function (ms, fn) {
    var self = this;
    var id = setInterval(function () { if (!self.stopped) fn(); else clearInterval(id); }, ms);
    this.timers.push(id);
    return id;
  };
  Runner.prototype.clear = function () {
    this.timers.forEach(function (id) { clearTimeout(id); clearInterval(id); });
    this.timers = [];
  };

  /* ── Live meeting transcript ─────────────────────────────────── */
  function Meeting(root) {
    this.root = root;
    this.body = root.querySelector("[data-transcript]");
    this.paras = Array.prototype.slice.call(root.querySelectorAll("template[data-para]"));
    this.wave = new Wave(root.querySelector("[data-wave]"));
    var timerEl = root.querySelector("[data-timer]");
    this.timer = timerEl ? new Timer(timerEl) : null;
    this.run = new Runner();
  }
  Meeting.prototype.renderAll = function () {
    var self = this;
    this.body.innerHTML = "";
    this.paras.forEach(function (tpl) { self.body.appendChild(self.node(tpl, tpl.innerHTML.trim(), "")); });
  };
  Meeting.prototype.node = function (tpl, committed, tentative) {
    var wrap = document.createElement("div");
    wrap.className = "tr-para";
    var meta = document.createElement("div");
    meta.className = "tr-meta";
    var who = document.createElement("span");
    who.className = "tr-who " + tpl.getAttribute("data-who");
    who.textContent = tpl.getAttribute("data-label");
    var at = document.createElement("span");
    at.className = "tr-at mono";
    at.textContent = tpl.getAttribute("data-at");
    meta.appendChild(who); meta.appendChild(at);
    var p = document.createElement("p");
    p.className = "tr-text";
    p.appendChild(document.createTextNode(committed));
    if (tentative) {
      var tail = document.createElement("span");
      tail.className = "tr-tentative";
      tail.textContent = tentative;
      p.appendChild(tail);
    }
    wrap.appendChild(meta); wrap.appendChild(p);
    return wrap;
  };
  /* The transcript grows by three paragraphs as it streams, which pushed
     everything below it down each time. Render the finished transcript once,
     measure it, and hold that height from the start. */
  Meeting.prototype.reserve = function () {
    this.body.style.minHeight = "";
    var restore = this.body.innerHTML;
    this.renderAll();
    var h = this.body.getBoundingClientRect().height;
    this.body.innerHTML = restore;
    if (h > 0) this.body.style.minHeight = Math.ceil(h) + "px";
  };
  Meeting.prototype.play = function () {
    var self = this;
    this.run.clear();
    this.run.stopped = false;
    this.body.innerHTML = "";
    if (this.timer) { this.timer.reset(); this.timer.start(); }
    this.wave.start(true);

    var index = 0;
    function nextParagraph() {
      if (index >= self.paras.length) {
        // Hold the finished transcript on screen, then run it again.
        self.run.after(4200, function () { self.play(); });
        return;
      }
      var tpl = self.paras[index];
      var all = words(tpl.innerHTML.trim());
      var node = self.node(tpl, "", "");
      self.body.appendChild(node);
      var p = node.querySelector(".tr-text");
      var i = 0;
      // The tail arrives tentative (half opacity) and commits a few
      // words behind, the way the live transcript store does.
      var id = self.run.every(105, function () {
        if (i >= all.length) {
          clearInterval(id);
          p.textContent = all.join("");
          index += 1;
          self.run.after(650, nextParagraph);
          return;
        }
        i += 1;
        var commitTo = Math.max(0, i - 6);
        p.textContent = all.slice(0, commitTo).join("");
        var tail = document.createElement("span");
        tail.className = "tr-tentative";
        tail.textContent = all.slice(commitTo, i).join("");
        p.appendChild(tail);
      });
    }
    nextParagraph();
  };
  Meeting.prototype.stop = function () {
    this.run.stopped = true; this.run.clear();
    this.wave.stop(); if (this.timer) this.timer.stop();
  };
  Meeting.prototype.still = function () {
    this.renderAll();
    if (this.timer) this.timer.render();
    this.wave.start(false);
    this.wave.stop();
  };

  /* ── Dictation surface ───────────────────────────────────────── */
  function Dictation(root) {
    this.root = root;
    this.target = root.querySelector("[data-typed]");
    this.text = this.target ? this.target.getAttribute("data-text") : "";
    this.wave = new Wave(root.querySelector("[data-wave]"));
    var timerEl = root.querySelector("[data-timer]");
    this.timer = timerEl ? new Timer(timerEl) : null;
    this.run = new Runner();
  }
  Dictation.prototype.reserve = function () {
    var p = this.target.parentNode;
    if (!p) return;
    p.style.minHeight = "";
    var restore = this.target.textContent;
    this.target.textContent = this.text;
    var h = p.getBoundingClientRect().height;
    this.target.textContent = restore;
    if (h > 0) p.style.minHeight = Math.ceil(h) + "px";
  };
  Dictation.prototype.play = function () {
    var self = this;
    this.run.clear(); this.run.stopped = false;
    this.target.textContent = "";
    if (this.timer) { this.timer.reset(); this.timer.start(); }
    this.wave.start(true);
    var all = words(this.text), i = 0;
    var id = this.run.every(95, function () {
      if (i >= all.length) {
        clearInterval(id);
        if (self.timer) self.timer.stop();
        self.wave.active = false;
        self.run.after(3600, function () { self.play(); });
        return;
      }
      i += 1;
      self.target.textContent = all.slice(0, i).join("");
    });
  };
  Dictation.prototype.stop = function () {
    this.run.stopped = true; this.run.clear();
    this.wave.stop(); if (this.timer) this.timer.stop();
  };
  Dictation.prototype.still = function () {
    this.target.textContent = this.text;
    if (this.timer) this.timer.render();
  };

  /* ── Overlay: the pill over a chat window ────────────────────── */
  function Overlay(root) {
    this.root = root;
    this.pill = root.querySelector("[data-pill]");
    this.label = root.querySelector("[data-pill-label]");
    this.dot = root.querySelector("[data-pill-dot]");
    this.stopBtn = root.querySelector("[data-pill-stop]");
    this.waveBox = root.querySelector("[data-pill-wave]");
    this.spinner = root.querySelector("[data-pill-spinner]");
    this.textEl = root.querySelector("[data-pill-text]");
    this.composer = root.querySelector("[data-composer]");
    this.input = root.querySelector("[data-input]");
    this.placeholder = root.querySelector("[data-ph]");
    this.send = root.querySelector("[data-send]");
    this.thread = root.querySelector("[data-thread]");
    this.steps = Array.prototype.slice.call(root.querySelectorAll("[data-step]"));
    this.keycast = root.querySelector("[data-keycast]");
    this.raw = root.querySelector("[data-raw]").textContent.trim();
    this.polished = root.querySelector("[data-polished]").textContent.trim();
    this.labels = {
      dictating: this.label.textContent.trim(),
      polishing: root.getAttribute("data-polishing") || ""
    };
    this.me = {
      name: root.getAttribute("data-me") || "",
      initials: root.getAttribute("data-me-initials") || "",
      colour: root.getAttribute("data-me-colour") || "#5b5bd6",
      at: root.getAttribute("data-me-at") || ""
    };
    this.wave = new Wave(root.querySelector("[data-pill-wave] [data-wave]"));
    this.run = new Runner();
    this.sent = null;
  }
  Overlay.prototype.setDraft = function (text) {
    this.input.textContent = text;
    if (this.placeholder) this.placeholder.hidden = text.length > 0;
  };
  Overlay.prototype.step = function (n) {
    this.steps.forEach(function (el, i) { el.classList.toggle("is-active", i === n); });
  };
  Overlay.prototype.reset = function () {
    this.pill.classList.remove("is-visible", "is-expanded");
    this.textEl.hidden = true; this.textEl.textContent = "";
    this.spinner.hidden = true;
    this.waveBox.hidden = false;
    this.dot.hidden = false;
    this.stopBtn.hidden = false;
    this.label.textContent = this.labels.dictating;
    if (this.keycast) this.keycast.classList.remove("is-on", "is-pressed");
    this.setDraft("");
    this.composer.classList.add("is-focused");
    this.send.classList.remove("is-ready");
    if (this.sent && this.sent.parentNode) this.sent.parentNode.removeChild(this.sent);
    this.sent = null;
    this.step(-1);
  };
  Overlay.prototype.appendSent = function (text) {
    var msg = document.createElement("div");
    msg.className = "chat-msg";
    msg.style.animation = "rise-in 240ms ease";
    var avatar = document.createElement("span");
    avatar.className = "chat-avatar";
    avatar.style.background = this.me.colour;
    avatar.textContent = this.me.initials;
    var body = document.createElement("div");
    body.className = "chat-body";
    var head = document.createElement("div");
    var who = document.createElement("span");
    who.className = "chat-name";
    who.textContent = this.me.name;
    var at = document.createElement("span");
    at.className = "chat-at";
    at.textContent = this.me.at;
    head.appendChild(who); head.appendChild(at);
    var p = document.createElement("p");
    p.className = "chat-text";
    p.textContent = text;
    body.appendChild(head); body.appendChild(p);
    msg.appendChild(avatar); msg.appendChild(body);
    this.thread.appendChild(msg);
    this.sent = msg;
  };
  Overlay.prototype.play = function () {
    var self = this;
    this.run.clear(); this.run.stopped = false;
    this.reset();

    // 1. the shortcut: flash the keys, depress them, and only then does the
    //    pill appear, so the trigger is legible rather than implied.
    this.run.after(300, function () {
      self.step(0);
      if (self.keycast) self.keycast.classList.add("is-on");
    });
    this.run.after(950, function () {
      if (self.keycast) self.keycast.classList.add("is-pressed");
    });
    this.run.after(1250, function () {
      if (self.keycast) self.keycast.classList.remove("is-on");
      self.pill.classList.add("is-visible");
      self.wave.start(true);
    });
    this.run.after(1600, function () {
      if (self.keycast) self.keycast.classList.remove("is-pressed");
    });

    // 2. speech arrives: the pill widens and the live text fills in.
    this.run.after(2150, function () {
      self.step(1);
      self.pill.classList.add("is-expanded");
      self.textEl.hidden = false;
      var all = words(self.raw), i = 0;
      var id = self.run.every(90, function () {
        if (i >= all.length) {
          clearInterval(id);
          // 3. stop: no dot, no stop button, a spinner while it reformulates.
          self.run.after(600, function () {
            self.step(2);
            self.wave.stop();
            self.dot.hidden = true;
            self.stopBtn.hidden = true;
            self.waveBox.hidden = true;
            self.spinner.hidden = false;
            self.label.textContent = self.labels.polishing;
            self.textEl.hidden = true;
            self.pill.classList.remove("is-expanded");
          });
          // 4. the pill goes away and the text lands in the composer.
          self.run.after(2400, function () {
            self.step(3);
            self.pill.classList.remove("is-visible");
            var out = words(self.polished), j = 0;
            var tid = self.run.every(28, function () {
              if (j >= out.length) {
                clearInterval(tid);
                self.send.classList.add("is-ready");
                self.run.after(900, function () {
                  self.appendSent(self.polished);
                  self.setDraft("");
                  self.send.classList.remove("is-ready");
                  self.run.after(3200, function () { self.play(); });
                });
                return;
              }
              j += 1;
              self.setDraft(out.slice(0, j).join(""));
            });
          });
          return;
        }
        i += 1;
        self.textEl.textContent = all.slice(0, i).join("");
      });
    });
  };
  Overlay.prototype.stop = function () {
    this.run.stopped = true; this.run.clear(); this.wave.stop();
  };
  Overlay.prototype.still = function () {
    this.reset();
    this.setDraft(this.polished);
    this.send.classList.add("is-ready");
    this.step(3);
  };

  /* ── Wiring: only animate what is on screen ──────────────────── */
  var scenes = [];
  document.querySelectorAll('[data-mock="meeting"]').forEach(function (el) { scenes.push(new Meeting(el)); });
  document.querySelectorAll('[data-mock="dictation"]').forEach(function (el) { scenes.push(new Dictation(el)); });
  document.querySelectorAll('[data-mock="overlay"]').forEach(function (el) { scenes.push(new Overlay(el)); });

  function reserveAll() {
    scenes.forEach(function (s) { if (s.reserve) s.reserve(); });
  }
  reserveAll();
  // Webfont metrics change the wrapping, so measure again once they land.
  if (document.fonts && document.fonts.ready) {
    document.fonts.ready.then(reserveAll).catch(function () {});
  }

  if (REDUCED || !("IntersectionObserver" in window)) {
    scenes.forEach(function (s) { s.still(); });
    return;
  }

  var io = new IntersectionObserver(function (entries) {
    entries.forEach(function (e) {
      var scene = e.target.__scene;
      if (!scene) return;
      if (e.isIntersecting) scene.play();
      else scene.stop();
    });
  }, { threshold: 0.25 });

  scenes.forEach(function (s) { s.root.__scene = s; io.observe(s.root); });

  var resizeId = 0;
  window.addEventListener("resize", function () {
    clearTimeout(resizeId);
    resizeId = setTimeout(function () {
      scenes.forEach(function (s) { if (s.wave) s.wave.resize(); });
      reserveAll();
    }, 150);
  });

  document.addEventListener("visibilitychange", function () {
    scenes.forEach(function (s) { if (document.hidden) s.stop(); });
  });
})();
