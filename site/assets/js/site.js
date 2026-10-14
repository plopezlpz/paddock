/* paddock site: the scroll-driven demo (sticky player, steps, one smooth timeline per recording), the drawn-in margin notes,
   install tabs and copy buttons. Classic script, no dependencies. Load order: frames files, player.js, this file (all defer);
   the demo boots on DOMContentLoaded. */
(function () {
  var live = document.getElementById('live');
  function say(msg) { if (live) live.textContent = msg; }

  var rmQ = window.matchMedia('(prefers-reduced-motion: reduce)');
  var wideQ = window.matchMedia('(min-width: 1100px)');
  /* The user pressed pause: in the wide layout it stays paused as the steps change, until they press play. */
  var held = { paused: false, wide: false };
  var FADE_MS = 120;   // the player's loop dip (out in 120 ms); the same dip softens a cut inside the tour
  var SWAP_MS = 120;   // a step must stay current this long before the stage player is swapped (a fast flick skips the ones in between)
  var WAS_MS = 260;    // the outgoing player stays solid under the incoming one this long

  function frames(rec) { return window.PADDOCK_FRAMES && window.PADDOCK_FRAMES[rec]; }

  /* ---- fingerprints: which frame is on screen? The player does not say, so a frame is recognised by its text (shape glyphs,
     which render as empty boxes, count as no text). Used to follow seeks, steps and loops without touching player.js. ---- */
  var SHAPE = /[\u2500\u2502\u251c\u2524\u252c\u2534\u253c\u250c\u2510\u2514\u2518\u256d\u256e\u2570\u256f\u258c\u258f]/g;
  var fpCache = {};
  function fingerprints(rec) {
    if (fpCache[rec]) return fpCache[rec];
    var d = frames(rec), lt = [];
    function line(li) {
      if (lt[li] === undefined) lt[li] = d.lines[li].map(function (r) { return r[1].replace(SHAPE, ''); }).join('');
      return lt[li];
    }
    return (fpCache[rec] = d.frames.map(function (f) { return f.l.map(line).join('\n'); }));
  }

  /* ---- clock + timeline: a thin track for one clip (a whole recording, or a run of its chapters), filled from elapsed time, not
     from the player's frame-by-frame bar. fill = (time into the clip) / (length of the clip), 0 to 100% and then it loops,
     redrawn every animation frame while playing. Only the chapter ticks inside the clip are shown. ---- */
  function Clock(ctl, rec) {
    var fig = ctl.fig, api = ctl.api, data = frames(rec), fr = data.frames, n = fr.length, chapters = data.chapters || [];
    var controls = fig.querySelector('.pd-controls'), btns = controls.querySelector('.pd-btns');
    var screen = fig.querySelector('.pd-screen'), playBtn = fig.querySelector('.pd-play'), chapBtns = controls.querySelectorAll('.pd-chap');
    var cum = [0], total = 0, fps = fingerprints(rec);
    fr.forEach(function (f, i) { total += f.d || 1000; cum[i + 1] = total; });
    function hold(i) { return fr[i].d || 1000; }

    var tl = document.createElement('div'), fill = document.createElement('i');
    tl.className = 'tl'; fill.className = 'fill';
    tl.appendChild(fill);
    var ticks = [];
    if (chapters.length > 1) chapters.forEach(function (c, i) {
      var b = document.createElement('button');
      b.type = 'button'; b.className = 'tick';
      b.title = c.title || c.id;
      b.addEventListener('click', function () { if (ctl.onTick) ctl.onTick(i); });
      tl.appendChild(b); ticks.push(b);
    });
    (btns || controls.firstChild).after(tl);

    var idx = 0, startEl = 0, rate = 1, t0 = 0, running = false, inView = !('IntersectionObserver' in window);
    var segFrom = 0, segTo = n, pending = -1, raf = 0, fadeT = 0, resumeT = 0;

    function wanted() {
      return !!playBtn && playBtn.getAttribute('aria-pressed') === 'true' && !rmQ.matches && inView && !document.hidden;
    }
    function elapsed(now) { return running ? Math.min(hold(idx), startEl + (now - t0) * rate) : startEl; }
    function draw(now) {
      var f = (cum[idx] + elapsed(now === undefined ? performance.now() : now) - cum[segFrom]) / ((cum[segTo] - cum[segFrom]) || 1);
      fill.style.transform = 'scaleX(' + Math.min(1, Math.max(0, f)) + ')';
    }
    function tick(now) {
      raf = running ? requestAnimationFrame(tick) : 0;
      draw(now);
    }

    /* The player restarts a frame's hold whenever it resumes (pause, scroll back into view), which would stretch that frame. So a
       frame resumed part-way is ended by a seek of our own after what is left of it, and the fill carries on at normal speed. */
    function refresh() {
      var now = performance.now(), run = wanted();
      if (run && !running) {
        var d = hold(idx);
        t0 = now; rate = 1; running = true;
        if (startEl > 0 && startEl < d) {
          if (idx + 1 < n) {
            var at = idx;
            clearTimeout(resumeT);
            resumeT = setTimeout(function () { if (running && idx === at) ctl.seek(at + 1); }, d - startEl);
          } else rate = (d - startEl) / d;   // the last frame: the player's loop decides when it ends, so pace to it
        }
        if (!raf) raf = requestAnimationFrame(tick);
      } else if (!run && running) {
        startEl = elapsed(now); running = false;
        tl.classList.remove('dip');
        clearTimeout(fadeT); fadeT = 0;
        clearTimeout(resumeT); resumeT = 0;
      }
      draw(now);
    }

    function resolve(text) {
      var exp = idx + 1 < n ? idx + 1 : 0, cands = [];
      if (pending >= 0) { var p = pending; pending = -1; return p; }
      for (var i = 0; i < n; i++) if (fps[i] === text) cands.push(i);
      if (!cands.length || cands.indexOf(exp) >= 0) return exp;
      return cands.reduce(function (best, c) { return Math.abs(c - idx) < Math.abs(best - idx) || (Math.abs(c - idx) === Math.abs(best - idx) && c > best) ? c : best; });
    }

    /* every render of the screen is a frame change (advance, seek, step button, loop) */
    function onRender() {
      var now = performance.now();
      tl.classList.remove('dip');   // the fill comes back in from 0 (draw below puts it there while it is still hidden)
      idx = resolve(screen.textContent);
      startEl = 0; rate = 1; t0 = now;
      clearTimeout(fadeT); fadeT = 0;
      clearTimeout(resumeT); resumeT = 0;
      if (wanted() && (idx < segFrom || idx >= segTo)) {   // the clip is over: loop it, with no sweep back
        ctl.seek(segFrom);
        idx = segFrom;
      } else if (wanted() && idx === segTo - 1) {
        /* the loop dip: the screen (for a clip that ends before the recording does; the player dips by itself at the end of the
           recording), the fill, and the caption and key of the clip's first frame, all while the picture is down */
        var cut = segTo < n;
        fadeT = setTimeout(function () {
          fadeT = 0;
          if (cut) screen.classList.add('pd-fading');
          tl.classList.add('dip');
          if (api.preroll) api.preroll(segFrom);
        }, Math.max(0, hold(idx) - (cut ? FADE_MS : 0)));
      }
      draw(now);
    }

    /* the clip the track shows and loops: frames [from, to). Ticks outside it are hidden, those inside are laid out along it. */
    function setSeg(from, to) {
      segFrom = from; segTo = to;
      var span = (cum[to] - cum[from]) || 1, inside = [];
      ticks.forEach(function (b, i) {
        var at = chapters[i].frame || 0;
        if (at >= from && at < to) inside.push(i); else b.style.display = 'none';
      });
      inside.forEach(function (i, k) {
        var b = ticks[i], c = chapters[i];
        b.style.display = inside.length > 1 ? '' : 'none';
        b.style.left = (100 * (cum[c.frame || 0] - cum[from]) / span) + '%';
        b.setAttribute('aria-label', (k + 1) + ' ' + (c.title || c.id));
        b.innerHTML = '<span><b>' + (k + 1) + '</b><span class="tt">' + (c.id || c.title) + '</span></span>';
      });
      draw();
    }
    function sync() {
      if (!ticks.length) return;
      [].forEach.call(chapBtns, function (cb, i) {
        if (ticks[i]) ticks[i].setAttribute('aria-current', cb.getAttribute('aria-current') === 'step' ? 'step' : 'false');
      });
      if (ctl.onChapter) ctl.onChapter(chapters[[].findIndex.call(ticks, function (t) { return t.getAttribute('aria-current') === 'step'; })]);
    }

    if (window.MutationObserver) {
      new MutationObserver(onRender).observe(screen, { childList: true });
      if (playBtn) new MutationObserver(refresh).observe(playBtn, { attributes: true, attributeFilter: ['aria-pressed'] });
      new MutationObserver(refresh).observe(fig, { attributes: true, attributeFilter: ['class'] });
      var chapGroup = controls.querySelector('.pd-chapters');
      if (chapGroup) new MutationObserver(sync).observe(chapGroup, { subtree: true, attributes: true, attributeFilter: ['aria-current'] });
    }
    if ('IntersectionObserver' in window) {
      new IntersectionObserver(function (es) { inView = es[es.length - 1].isIntersecting; refresh(); }, { threshold: 0.15 }).observe(fig);
    }
    document.addEventListener('visibilitychange', refresh);
    var onRm = function () { refresh(); };
    if (rmQ.addEventListener) rmQ.addEventListener('change', onRm); else rmQ.addListener(onRm);
    sync();
    setSeg(0, n);
    return {
      setSeg: setSeg,
      seekTo: function (i) { pending = i; },      // the next render is frame i (the caller is about to seek there)
      frameAt: function () { return idx; },
      state: function () { return { idx: idx, running: running, total: total, elapsed: elapsed(performance.now()) }; }
    };
  }

  /* ---- one recording as a player: built from markup, mounted by player.js, timed by Clock ---- */
  var KIND_PHONE = 'phone';
  var DOT_NOTE = '<div class="dot-note" aria-hidden="true"><svg viewBox="0 0 120 62" focusable="false"><path class="pen2" pathLength="1" d="M107 14 C 86 11, 56 24, 34 54"/><path pathLength="1" d="M105 11 C 84 8, 52 20, 31 55"/><path class="head" pathLength="1" d="M43 51 Q 35 55 31 58 Q 31.5 51 31.7 44"/></svg><span class="hand">agent state: orange while it works, blue once it is done</span></div>';
  function makeFig(rec, note) {
    var f = document.createElement('figure');
    f.className = 'pd-player';
    f.setAttribute('data-frames', rec);
    f.setAttribute('data-kind', rec === KIND_PHONE ? 'phone' : 'desktop');
    f.setAttribute('data-loop', '');
    f.setAttribute('data-keys', 'overlay');
    f.innerHTML = '<div class="pd-window"><div class="pd-bar" aria-hidden="true"><i></i><i></i><i></i><span>tmux</span></div><div class="pd-term"><pre class="pd-screen" aria-hidden="true"></pre><div class="pd-hud" aria-hidden="true"></div>' + (note ? DOT_NOTE : '') + '</div></div><div class="pd-controls"></div><figcaption class="pd-cap"></figcaption>';
    return f;
  }
  function mountFig(rec, into, note) {
    var fig = makeFig(rec, note);
    into.appendChild(fig);
    var ctl = { rec: rec, fig: fig, api: null, clock: null, onTick: null, onChapter: null, fadeT: 0 };
    var pressed = function () { var b = fig.querySelector('.pd-play'); return !!b && b.getAttribute('aria-pressed') === 'true'; };
    fig.addEventListener('click', function (e) {   // after the player's own handler: what the user left it as
      if (e.target.closest('.pd-play, .pd-prev, .pd-next')) held.paused = !pressed();
    });
    fig.addEventListener('keydown', function (e) {
      if (e.target.classList.contains('pd-term') && (e.key === ' ' || e.key === 'k' || e.key === 'K') && !e.ctrlKey && !e.metaKey && !e.altKey) held.paused = !pressed();
    });
    ctl.ready = window.PaddockPlayer.get(fig).ready.then(function (api) {
      if (!api) return null;
      ctl.api = api;
      ctl.screen = fig.querySelector('.pd-screen');
      ctl.clock = Clock(ctl, rec);
      /* seek and tell the clock which frame the next render is; a paused demo stays paused */
      ctl.seek = function (i) { ctl.clock.seekTo(i); api.seek(i); if (!(held.wide && held.paused)) api.play(); };
      return ctl;
    });
    return ctl;
  }

  /* ---- the demo ---- */
  function demo() {
    var root = document.getElementById('features'), stage = document.getElementById('stage');
    if (!root || !stage || !window.PaddockPlayer) return;
    var stepEls = [].slice.call(root.querySelectorAll('.step'));
    /* a step is one clip: a recording, or a run of its chapters (data-ch to data-ch-to). A step whose own recording is missing
       falls back to data-fb-rec / data-fb-ch. */
    var steps = stepEls.map(function (el) {
      var s = { el: el, rec: el.getAttribute('data-rec'), ch: el.getAttribute('data-ch'), chTo: el.getAttribute('data-ch-to'), dot: el.hasAttribute('data-dot') };
      if (!frames(s.rec) && el.getAttribute('data-fb-rec')) { s.rec = el.getAttribute('data-fb-rec'); s.ch = el.getAttribute('data-fb-ch'); s.chTo = null; }
      return s;
    }).filter(function (s) { return frames(s.rec); });
    steps.forEach(function (s) {
      var d = frames(s.rec), chs = d.chapters || [], a = -1, b = -1;
      chs.forEach(function (c, i) { if (c.id === s.ch) a = i; if (c.id === (s.chTo || s.ch)) b = i; });
      s.ci = a;
      s.clip = a < 0 ? { from: 0, to: d.frames.length } : { from: chs[a].frame || 0, to: chs[b + 1] ? chs[b + 1].frame : d.frames.length };
    });
    var mode = '', cur = -1, shown = -1, lock = false, lockT = 0, swapT = 0, drawT = 0, stageSeen = !('IntersectionObserver' in window), stageCtl = {}, inline = [];

    /* every step plays and loops its own clip, and its progress bar runs 0 to 100% over exactly that clip */
    function begin(ctl, s, fade) {
      ctl.ready.then(function (c) {
        if (!c) return;
        var go = function () {
          ctl.fadeT = 0;
          c.clock.setSeg(s.clip.from, s.clip.to);
          if (rmQ.matches) { if (s.ci >= 0) c.api.chapter(s.ci); return; }
          c.seek(s.clip.from);
        };
        clearTimeout(ctl.fadeT);
        if (fade && !rmQ.matches) { c.screen.classList.add('pd-fading'); ctl.fadeT = setTimeout(go, FADE_MS); } else go();
      });
    }

    /* ---- sticky stage: one player per recording, cross-faded ---- */
    function ensureStage() {
      steps.forEach(function (s) {
        if (stageCtl[s.rec]) return;
        var ctl = stageCtl[s.rec] = mountFig(s.rec, stage, steps.some(function (st) { return st.dot && st.rec === s.rec; }));
        ctl.onTick = function (i) { held.paused = false; ctl.ready.then(function (c) { c.seek(frames(s.rec).chapters[i].frame || 0); }); };
        hookCards(ctl, s.rec);
      });
    }
    /* the step text changes at once; the stage player follows once the step has stayed current for SWAP_MS (or at once for a click) */
    function activate(i, now) {
      if (i === cur || !steps[i]) return;
      cur = i;
      steps.forEach(function (st, j) {
        st.el.classList.toggle('on', j === i);
        if (j === i) st.el.setAttribute('aria-current', 'step'); else st.el.removeAttribute('aria-current');
      });
      announce(i);
      clearTimeout(swapT); swapT = 0;
      if (now || lock || shown < 0) swapStage(i);
      else swapT = setTimeout(function () { swapT = 0; swapStage(i); }, SWAP_MS);
    }
    /* a polite announcement of the current step (the heading), once the reader has stayed on it for a moment */
    var annT = 0, stepLive = document.getElementById('step-live');
    function announce(i) {
      clearTimeout(annT);
      if (!stepLive || mode !== 'wide') return;
      annT = setTimeout(function () {
        var h = steps[i].el.querySelector('h3');
        stepLive.textContent = 'Step ' + (i + 1) + ' of ' + steps.length + ': ' + (h ? h.textContent : '');
      }, 600);
    }
    function isPhone(rec) { return rec === KIND_PHONE; }
    function endWas(ctl) {
      clearTimeout(ctl.wasT); ctl.wasT = 0;
      var cl = ctl.fig.classList;
      if (cl.contains('was')) cl.remove('was');
      if (cl.contains('x')) cl.remove('x');
      if (cl.contains('drawn') && !cl.contains('on')) cl.remove('drawn');
    }
    /* Swap the stage player: the outgoing one stays solid (.was) under the incoming one (.on) while that fades in. At most two
       players exist: any older outgoing one, and any transition still running, is ended first. */
    function swapStage(i) {
      if (i === shown || !steps[i]) return;
      var s = steps[i], prev = steps[shown], inc = stageCtl[s.rec], out = prev ? stageCtl[prev.rec] : null;
      shown = i;
      var kindChange = !!prev && isPhone(prev.rec) !== isPhone(s.rec);
      Object.keys(stageCtl).forEach(function (rec) {
        var ctl = stageCtl[rec];
        if (ctl === inc || ctl === out) return;
        endWas(ctl);
        ctl.fig.classList.remove('on', 'enter');
        ctl.ready.then(function (c) { if (c) { clearTimeout(ctl.fadeT); c.api.pause(); } });
      });
      if (out && out !== inc) {
        var cl = out.fig.classList;
        if (cl.contains('on')) cl.remove('on');
        cl.remove('enter');
        cl.add('was');
        if (kindChange) cl.add('x');
        clearTimeout(out.wasT);
        out.wasT = setTimeout(function () { endWas(out); }, WAS_MS);
        out.ready.then(function (c) { if (c) { clearTimeout(out.fadeT); c.api.pause(); } });
      }
      endWas(inc);
      inc.fig.classList.remove('enter');
      if (kindChange && isPhone(s.rec)) inc.fig.classList.add('enter');
      if (!inc.fig.classList.contains('on')) inc.fig.classList.add('on');
      begin(inc, s, !!prev && prev.rec === s.rec);
      maybeDraw();
    }
    /* the handwritten note draws in each visit: when the stage is at least half in view and its step is shown, 250 ms after the fade */
    function maybeDraw() {
      clearTimeout(drawT); drawT = 0;
      var s = steps[shown];
      if (mode !== 'wide' || !s || !s.dot || !stageSeen) return;
      var cl = stageCtl[s.rec].fig.classList;
      if (cl.contains('drawn')) return;
      drawT = setTimeout(function () {
        drawT = 0;
        var t = steps[shown];
        if (t && t.dot && stageSeen && !cl.contains('drawn')) cl.add('drawn');
      }, 250);
    }
    if ('IntersectionObserver' in window) {
      new IntersectionObserver(function (es) { stageSeen = es[es.length - 1].isIntersecting; if (stageSeen) maybeDraw(); }, { threshold: 0.5 }).observe(stage);
    }
    function goTo(i) {
      lock = true;
      activate(i, true);
      clearTimeout(lockT);
      var last = -1, settle = function () {
        var y = window.pageYOffset;
        if (y !== last) { last = y; lockT = setTimeout(settle, 120); } else lock = false;
      };
      window.scrollTo({ top: steps[i].el.getBoundingClientRect().top + window.pageYOffset - stageTop(), behavior: rmQ.matches ? 'auto' : 'smooth' });
      lockT = setTimeout(settle, 120);
    }

    /* One offset for three things: the sticky stage's top edge (CSS `top` of .sc-col), the line a step crosses to become active,
       and where goTo scrolls a step to. So the active step's heading lines up with the top of the player. */
    function stageTop() { return parseFloat(getComputedStyle(root.querySelector('.sc-col')).top) || 0; }

    /* ---- stacked: every step has its own player ---- */
    function ensureInline() {
      if (inline.length) return;
      steps.forEach(function (s) {
        var slot = s.el.querySelector('[data-slot]');
        var ctl = mountFig(s.rec, slot, s.dot);
        hookCards(ctl, s.rec);
        ctl.onTick = function (i) {
          ctl.ready.then(function (c) { if (rmQ.matches) c.api.chapter(i); else c.seek(frames(s.rec).chapters[i].frame || 0); });
        };
        s.ctl = ctl;
        inline.push(ctl);
      });
    }

    /* the notification cards beside the notify step follow the recording's chapter */
    function hookCards(ctl, rec) {
      if (rec !== 'feat-notify') return;
      var notif = document.querySelector('#features .notif');
      if (!notif) return;
      var cards = notif.querySelectorAll('.nc');
      ctl.onChapter = function (c) {
        var id = c ? c.id : '';
        notif.classList.toggle('has-cur', id === 'asking' || id === 'finished');
        [].forEach.call(cards, function (el) { el.classList.toggle('on', el.getAttribute('data-for') === id); });
      };
    }

    function setMode(m) {
      if (m === mode) return;
      mode = m;
      held.wide = m === 'wide'; held.paused = false;
      root.classList.toggle('is-wide', m === 'wide');
      root.classList.toggle('is-stack', m === 'stack');
      Object.keys(stageCtl).forEach(function (rec) { stageCtl[rec].ready.then(function (c) { if (c) c.api.pause(); }); });
      inline.forEach(function (ctl) { ctl.ready.then(function (c) { if (c) c.api.pause(); }); });
      stepEls.forEach(function (el) { if (m === 'wide') el.setAttribute('tabindex', '0'); else el.removeAttribute('tabindex'); });
      clearTimeout(swapT); swapT = 0;
      Object.keys(stageCtl).forEach(function (rec) { endWas(stageCtl[rec]); stageCtl[rec].fig.classList.remove('on', 'enter', 'drawn'); });
      shown = -1;
      if (m === 'wide') {
        ensureStage();
        var i = Math.max(cur, 0); cur = -1;
        activate(i, true);
      } else {
        ensureInline();
        steps.forEach(function (s) { s.el.classList.remove('on'); s.el.removeAttribute('aria-current'); });
        steps.forEach(function (s) { begin(s.ctl, s, false); });
        cur = -1;
      }
    }
    function modeNow() { return wideQ.matches && !rmQ.matches ? 'wide' : 'stack'; }
    function onModeChange() { setMode(modeNow()); }
    [wideQ, rmQ].forEach(function (q) { if (q.addEventListener) q.addEventListener('change', onModeChange); else q.addListener(onModeChange); });

    /* ---- activation: the current step is the last one whose heading has come up into the lower part of the viewport,
       so the visuals change as soon as the next title starts scrolling up. Above the section that is the first step, below it the last. ---- */
    var pickRaf = 0;
    function pick() {
      pickRaf = 0;
      if (mode !== 'wide' || lock) return;
      var line = Math.max(stageTop() + 40, window.innerHeight * 0.85), k = 0;
      steps.forEach(function (s, j) { if (s.el.getBoundingClientRect().top <= line) k = j; });
      activate(k);
    }
    function queuePick() { if (!pickRaf) pickRaf = requestAnimationFrame(pick); }
    window.addEventListener('scroll', queuePick, { passive: true });
    window.addEventListener('resize', queuePick);

    steps.forEach(function (s, i) {
      s.el.addEventListener('click', function (e) {
        if (mode !== 'wide' || i === cur || e.target.closest('a, button')) return;
        goTo(i);
      });
      s.el.addEventListener('keydown', function (e) {
        if (mode === 'wide' && e.target === s.el && (e.key === 'Enter' || e.key === ' ')) { e.preventDefault(); goTo(i); }
      });
    });

    window.paddockDemo = { steps: steps, active: function () { return cur; }, mode: function () { return mode; }, stage: stageCtl, inline: inline };
    setMode(modeNow());
  }

  /* ---- install tabs (WAI-ARIA tabs pattern, automatic activation) ---- */
  var tablist = document.querySelector('.inst [role="tablist"]');
  if (tablist) {
    var tabs = Array.prototype.slice.call(tablist.querySelectorAll('[role="tab"]'));
    var select = function (tab, focus) {
      tabs.forEach(function (other) {
        var on = other === tab;
        var panel = document.getElementById(other.getAttribute('aria-controls'));
        other.setAttribute('aria-selected', on ? 'true' : 'false');
        other.tabIndex = on ? 0 : -1;
        if (panel) { panel.classList.toggle('is-active', on); }
      });
      if (focus) tab.focus();
    };
    tabs.forEach(function (tab, i) {
      tab.addEventListener('click', function () { select(tab, false); });
      tab.addEventListener('keydown', function (e) {
        var to = null;
        if (e.key === 'ArrowRight') to = tabs[(i + 1) % tabs.length];
        else if (e.key === 'ArrowLeft') to = tabs[(i - 1 + tabs.length) % tabs.length];
        else if (e.key === 'Home') to = tabs[0];
        else if (e.key === 'End') to = tabs[tabs.length - 1];
        if (to) { e.preventDefault(); select(to, true); }
      });
    });
  }

  /* ---- copy buttons: Clipboard API, then execCommand, then "select it yourself" ---- */
  function selectText(el) {
    var sel = window.getSelection();
    var range = document.createRange();
    range.selectNodeContents(el);
    sel.removeAllRanges();
    sel.addRange(range);
  }
  function flash(btn, label, state) {
    var orig = btn.getAttribute('data-label') || btn.textContent;
    btn.setAttribute('data-label', orig);
    btn.textContent = label;
    btn.setAttribute('data-state', state);
    clearTimeout(btn._t);
    btn._t = setTimeout(function () { btn.textContent = orig; btn.removeAttribute('data-state'); }, 2200);
  }
  var isMac = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || '');
  function copy(btn) {
    var code = document.getElementById(btn.getAttribute('data-copy'));
    if (!code) return;
    var text = code.textContent;
    var done = function () { flash(btn, 'copied', 'done'); say('Copied to the clipboard'); };
    var manual = function () {
      selectText(code);
      var msg = 'press ' + (isMac ? '⌘C' : 'Ctrl+C');
      flash(btn, msg, 'manual');
      say('Selected. ' + msg.replace('press', 'Press') + ' to copy.');
    };
    var legacy = function () {
      selectText(code);
      var ok = false;
      try { ok = document.execCommand('copy'); } catch (e) {}
      if (ok) done(); else manual();
    };
    if (navigator.clipboard && navigator.clipboard.writeText) {
      var p;
      try { p = navigator.clipboard.writeText(text); } catch (e) { legacy(); return; }
      p.then(done, legacy);
    } else {
      legacy();
    }
  }
  var copyBtns = document.querySelectorAll('[data-copy]');
  for (var c = 0; c < copyBtns.length; c++) {
    copyBtns[c].addEventListener('click', function (e) { copy(e.currentTarget); });
  }

  /* ---- margin notes draw in when their row scrolls into view ---- */
  function notes() {
    var rows = document.querySelectorAll('.cmp tr[data-note]');
    if (!('IntersectionObserver' in window)) { [].forEach.call(rows, function (r) { r.classList.add('drawn'); }); return; }
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (en) {
        if (!en.isIntersecting) return;
        var row = en.target;
        var i = [].indexOf.call(rows, row);
        setTimeout(function () { row.classList.add('drawn'); }, (i % 5) * 90);
        io.unobserve(row);
      });
    }, { threshold: 0.6 });
    [].forEach.call(rows, function (r) { io.observe(r); });
  }


  /* deferred scripts run before DOMContentLoaded */
  if (document.readyState === 'complete') demo(); else document.addEventListener('DOMContentLoaded', demo);

  var booted = false;
  window.addEventListener('load', function () {
    if (booted) return;
    booted = true;
    notes();
    /* the terminal is measured in JetBrains Mono: once the font is in, ask the player to measure again (it listens for load) */
    if (document.fonts && document.fonts.load) {
      Promise.all([document.fonts.load('400 14px "JetBrains Mono"'), document.fonts.load('700 14px "JetBrains Mono"')]).then(function () {
        window.dispatchEvent(new Event('load'));
      });
    }
  });

})();
