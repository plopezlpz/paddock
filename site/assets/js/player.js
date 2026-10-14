/* paddock terminal player: classic script, no dependencies.
   Renders frame data (window.PADDOCK_FRAMES[name], format v1) into a <pre>.
   Load order: frames files, then this file (defer). API: PaddockPlayer.mount(el, opts), PaddockPlayer.get(elOrId, opts).
   opts.keys (or data-keys on the figure): 'row' (default) shows the key pressed in a row under the terminal; 'overlay' casts
   keycaps over the terminal's bottom-right corner, like a screencast. */
(function () {
  'use strict';
  if (window.PaddockPlayer) return;

  var LINE_H = 1.2;          // line height in em; rounded to whole pixels per size
  var FIT = { desktop: [9, 14], phone: [6, 14], static: [8, 14] };   // font-size clamps in px
  // Below this effective size a scaled desktop demo scrolls sideways instead. Deliberately low: a scaled live
  // overview that shows the popup's structure beats sideways scrolling on a phone, and a tap enlarges it.
  var MIN_SCALED_FS = 3;
  var SMALL_FS = 7;          // a desktop demo scaled below this effective size offers an enlarged view
  var ZOOM_FS = 11.5;        // and that view draws the terminal at this size, scrollable
  var MAX_KEY_MS = 1200;
  var FADE_MS = 120;         // the loop dip: out in 120 ms, back in over 200 ms (CSS)
  var KEY_LEAD = 160;        // a keycap shows this long before the frame its key produces
  var CAP_LEAD = 150;        // and a new caption starts changing this long before its frame
  var TAP_MS = 500;          // tap ring duration; it starts this long before the frame that shows the tap's result

  var players = new WeakMap();
  var rmQuery = window.matchMedia('(prefers-reduced-motion: reduce)');
  var ratio = 0;

  var ICONS = {
    play: '<svg class="i-play" viewBox="0 0 16 16" aria-hidden="true"><path d="M4 2.5v11l9-5.5z"/></svg>' +
      '<svg class="i-pause" viewBox="0 0 16 16" aria-hidden="true"><path d="M3.5 2.5h3v11h-3zM9.5 2.5h3v11h-3z"/></svg>',
    prev: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3 2.5h2v11H3zM13 2.5v11L6 8z"/></svg>',
    next: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M11 2.5h2v11h-2zM3 2.5l7 5.5-7 5.5z"/></svg>'
  };

  function esc(s) { return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;'); }
  function isHex(s) { return typeof s === 'string' && /^#[0-9a-fA-F]{6}$/.test(s); }
  function el(tag, cls, attrs) {
    var n = document.createElement(tag);
    if (cls) n.className = cls;
    if (attrs) for (var k in attrs) n.setAttribute(k, attrs[k]);
    return n;
  }

  /* Width of one cell as a fraction of the font size, measured from a 100-character span. */
  function cellRatio() {
    if (ratio) return ratio;
    var s = el('span', 'pd-measure');
    s.textContent = new Array(101).join('0');
    document.body.appendChild(s);
    var w = s.getBoundingClientRect().width;
    s.remove();
    ratio = w / 100 / 100;
    if (!(ratio > 0.3 && ratio < 1)) ratio = 0.6;
    return ratio;
  }

  /* ---- frame data to HTML ---- */

  function styleInfo(st) {
    var cls = [], css = '', bg = false, v;
    v = st.fg;
    if (typeof v === 'number' && v >= 0 && v < 16) cls.push('f' + v);
    else if (v === 'fg') cls.push('ffg');
    else if (v === 'bg') cls.push('fbg');
    else if (isHex(v)) css += 'color:' + v + ';--c:' + v + ';';
    v = st.bg;
    if (typeof v === 'number' && v >= 0 && v < 16) { cls.push('k' + v); bg = true; }
    else if (v === 'fg') { cls.push('kfg'); bg = true; }
    else if (v === 'bg') { cls.push('kbg'); bg = true; }
    else if (isHex(v)) { css += 'background:' + v + ';--b:' + v + ';'; bg = true; }
    if (st.b) cls.push('B');
    if (st.d) cls.push('D');
    if (st.i) cls.push('I');
    if (st.u) cls.push('U');
    if (st.s) cls.push('S');
    return { cls: cls.join(' '), css: css, bg: bg };
  }

  /* Glyphs the UI draws structure with are drawn as cell-filling shapes (see .sh in player.css), so they
     join across rows and columns whatever the font and line height do. Class suffix = shape. */
  var SHAPES = {
    '\u2500': 'h', '\u2502': 'v', '\u251c': 'vr', '\u2524': 'vl', '\u252c': 'hd', '\u2534': 'hu', '\u253c': 'x',
    '\u250c': 'dr', '\u2510': 'dl', '\u2514': 'ur', '\u2518': 'ul',
    '\u256d': 'rdr', '\u256e': 'rdl', '\u2570': 'rur', '\u256f': 'rul',
    '\u258c': 'half', '\u258f': 'eighth'
  };

  /* ASCII stays plain text; every other glyph gets its own one-cell box. */
  function textHtml(t) {
    var out = '', buf = '', n = 0, chars = Array.from(t);
    for (var i = 0; i < chars.length; i++) {
      var ch = chars[i];
      n++;
      if (ch.charCodeAt(0) < 0x80) { buf += ch; continue; }
      if (buf) { out += esc(buf); buf = ''; }
      out += SHAPES[ch] ? '<i class="g sh sh-' + SHAPES[ch] + '"></i>' : '<i class="g">' + ch + '</i>';
    }
    if (buf) out += esc(buf);
    return { html: out, n: n };
  }

  function cropRuns(runs, c0, c1) {
    var out = [], pos = 0;
    for (var i = 0; i < runs.length; i++) {
      var chars = Array.from(runs[i][1]), a = pos, b = pos + chars.length;
      pos = b;
      if (b <= c0 || a >= c1) continue;
      out.push([runs[i][0], chars.slice(Math.max(c0, a) - a, Math.min(c1, b) - a).join('')]);
      if (b >= c1) break;
    }
    return out;
  }

  function keyHtml(label) {
    var m = /^prefix\s+(.+)$/.exec(label);
    if (m) {
      return '<kbd>ctrl-b</kbd> ' + m[1].split(/\s+/).map(function (k) { return '<kbd>' + esc(k) + '</kbd>'; }).join(' ');
    }
    if (label.length <= 10) return '<kbd>' + esc(label) + '</kbd>';
    return '<span>' + esc(label) + '</span>';
  }

  /* keycast overlay: a label becomes one group of keycaps. 'prefix a' is two caps, '^g' is ctrl + g, '^g' style labels and
     glyph labels ('⏎ Enter', '↓') are otherwise one cap each. Typed text ('c', 'ca', 'car') is a single cap that grows. */
  var KC_KEEP = 3;
  function isTyped(label) { return /^[\x21-\x7e]+$/.test(label) && label.charAt(0) !== '^'; }
  function capsHtml(label) {
    var parts, m;
    if ((m = /^prefix\s+(.+)$/.exec(label))) parts = ['prefix'].concat(m[1].split(/\s+/));
    else if ((m = /^shift\s+(.+)$/.exec(label))) parts = ['shift', m[1]];
    else if ((m = /^\^(.+)$/.exec(label))) parts = ['ctrl', m[1]];
    else parts = [label];
    return parts.map(function (k) { return '<kbd>' + esc(k) + '</kbd>'; }).join('');
  }

  function truthy(root, name) {
    if (!root.hasAttribute('data-' + name)) return false;
    var v = root.getAttribute('data-' + name);
    return v !== 'false' && v !== '0';
  }

  /* ---- one player ---- */

  function create(root, opts) {
    var staticSpec = /^([^:]+):(\d+)$/.exec(root.getAttribute('data-static') || '');
    var isStatic = root.hasAttribute('data-static');
    var name = staticSpec ? staticSpec[1] : root.getAttribute('data-frames');
    var kind = root.getAttribute('data-kind') === 'phone' ? 'phone' : 'desktop';
    root.setAttribute('data-kind', kind);

    var api = { frameCount: 0 };
    api.ready = new Promise(function (resolve) { api._resolve = resolve; });

    var data = window.PADDOCK_FRAMES && window.PADDOCK_FRAMES[name];
    if (!data) {
      // frames script may still be arriving; one retry on load, then give up quietly
      var retried = root.__pdRetried;
      root.__pdRetried = true;
      if (!retried && document.readyState !== 'complete') {
        window.addEventListener('load', function () { players.delete(root); mount(root); });
      } else {
        root.classList.add('pd-failed');
        console.warn('paddock player: no frames named "' + name + '"');
      }
      api._resolve(null);
      return api;
    }
    if (data.v !== 1) console.warn('paddock player: unexpected frame format v' + data.v);

    var frames = data.frames, chapters = data.chapters || [], n = frames.length;
    api.frameCount = n;

    /* ---- DOM ---- */
    var win = root.querySelector('.pd-window');
    if (!win) { win = el('div', 'pd-window'); root.insertBefore(win, root.firstChild); }
    var bar = win.querySelector('.pd-bar');
    if (!bar && kind === 'desktop') {
      bar = el('div', 'pd-bar', { 'aria-hidden': 'true' });
      bar.innerHTML = '<i></i><i></i><i></i><span>tmux</span>';
      win.insertBefore(bar, win.firstChild);
    }
    var term = win.querySelector('.pd-term');
    if (!term) { term = el('div', 'pd-term'); win.appendChild(term); }
    var screen = term.querySelector('.pd-screen');
    if (!screen) { screen = el('pre', 'pd-screen', { 'aria-hidden': 'true' }); term.appendChild(screen); }
    var hud = term.querySelector('.pd-hud');
    if (!hud) { hud = el('div', 'pd-hud', { 'aria-hidden': 'true' }); term.appendChild(hud); }
    hud.innerHTML = '<div class="pd-tapbox"></div>';
    var tapbox = hud.firstChild;
    var overlay = ((opts && opts.keys) || root.getAttribute('data-keys')) === 'overlay';
    var kc = null, kcGroups = [], kcClearT = 0;
    if (overlay) { kc = el('div', 'pd-kc'); hud.appendChild(kc); }
    var what = kind === 'phone' ? 'phone demo' : 'desktop demo';

    // the stage wraps the window so it can be scaled without touching the controls;
    // the scroller inside it is the horizontal scroll container when the demo does not fit
    var stage = el('div', 'pd-stage');
    var scroller = el('div', 'pd-scroller');
    win.parentNode.insertBefore(stage, win);
    stage.appendChild(scroller);
    scroller.appendChild(win);

    // keys pressed are shown outside the terminal so the screen renders exactly as recorded
    var keyRow = null, keyChip = null, scrollHint = null;
    if (!isStatic) {
      scrollHint = el('div', 'pd-scrollhint', { 'aria-hidden': 'true' });
      scrollHint.textContent = 'scroll \u2192';
      stage.after(scrollHint);
      if (!overlay && frames.some(function (f) { return f.key; })) {
        keyRow = el('div', 'pd-keyrow', { 'aria-hidden': 'true' });
        keyChip = el('div', 'pd-key');
        keyRow.appendChild(keyChip);
        scrollHint.after(keyRow);
      }
    }

    var controls = null, playBtn, prevBtn, nextBtn, progress, chapBtns = [], cap = null, zoomBtn = null, zoomClose = null;
    if (!isStatic) {
      term.setAttribute('tabindex', '0');
      term.setAttribute('role', 'group');
      controls = root.querySelector('.pd-controls');
      if (!controls) { controls = el('div', 'pd-controls'); (keyRow || scrollHint).after(controls); }
      controls.innerHTML = '';
      progress = el('div', 'pd-progress', { 'aria-hidden': 'true' });
      progress.appendChild(document.createElement('i'));
      var btns = el('div', 'pd-btns');
      playBtn = el('button', 'pd-btn pd-play', { type: 'button', 'aria-label': 'Play the ' + what, 'aria-pressed': 'false' });
      playBtn.innerHTML = ICONS.play;
      prevBtn = el('button', 'pd-btn pd-prev', { type: 'button', 'aria-label': 'Previous step in the ' + what });
      prevBtn.innerHTML = ICONS.prev;
      nextBtn = el('button', 'pd-btn pd-next', { type: 'button', 'aria-label': 'Next step in the ' + what });
      nextBtn.innerHTML = ICONS.next;
      btns.appendChild(playBtn); btns.appendChild(prevBtn); btns.appendChild(nextBtn);
      controls.appendChild(progress); controls.appendChild(btns);
      if (chapters.length) {
        var chaps = el('div', 'pd-chapters', { role: 'group', 'aria-label': 'Chapters' });
        chapters.forEach(function (c, i) {
          var b = el('button', 'pd-btn pd-chap', { type: 'button', 'aria-label': (i + 1) + ' ' + (c.title || c.id || '') + ', ' + what });
          b.innerHTML = '<span class="n">' + (i + 1) + '</span> ' + esc(c.title || c.id || '');
          b.addEventListener('click', function () { jump(i); });
          chaps.appendChild(b);
          chapBtns.push(b);
        });
        controls.appendChild(chaps);
      }
      cap = root.querySelector('.pd-cap');
      if (!cap) { cap = el('figcaption', 'pd-cap'); controls.after(cap); }
      if (kind === 'desktop') {
        // a desktop demo scaled down to a narrow screen: a tap opens it at a readable size
        zoomBtn = el('button', 'pd-zoom', { type: 'button', 'aria-label': 'Enlarge the ' + what, title: 'Enlarge' });
        zoomBtn.innerHTML = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M9.5 2H14v4.5M6.5 14H2V9.5M14 2 9 7M2 14l5-5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>';
        zoomClose = el('button', 'pd-zoom-close', { type: 'button', 'aria-label': 'Close the enlarged ' + what });
        zoomClose.innerHTML = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3 3l10 10M13 3 3 13" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/></svg><span>Close</span>';
        controls.appendChild(zoomBtn);   // in the controls row, not over the terminal
        stage.appendChild(zoomClose);
      }
    }

    /* ---- precomputed tables ---- */
    var styles = data.styles.map(styleInfo);
    var lineCache = [];
    var cum = [0];
    frames.forEach(function (f, i) { cum[i + 1] = cum[i] + (f.d || 1000); });
    var chOf = frames.map(function (f, i) {
      if (typeof f.ch === 'number') return f.ch;
      var c = -1;
      chapters.forEach(function (ch, k) { if (ch.frame <= i) c = k; });
      return Math.max(c, 0);
    });
    function posterOf(c) {
      var ch = chapters[c];
      if (!ch) return 0;
      return typeof ch.poster === 'number' ? ch.poster : ch.frame || 0;
    }

    var crop = null;
    if (isStatic && root.hasAttribute('data-crop')) {
      var cr = root.getAttribute('data-crop').split(',').map(Number);   // r0,r1,c0,c1 (end exclusive)
      if (cr.length === 4 && cr.every(isFinite)) crop = cr;
    }
    var cols = crop ? crop[3] - crop[2] : data.cols, rows = crop ? crop[1] - crop[0] : data.rows;
    root.style.setProperty('--pd-cols', cols);
    root.style.setProperty('--pd-rows', rows);

    function runHtml(r) {
      var info = styles[r[0]] || styles[0], t = textHtml(r[1]);
      if (!info.cls && !info.css) return t.html;
      if (info.bg) {
        return '<span class="' + info.cls + ' bgr" style="' + info.css + '--n:' + t.n + '">' + t.html + '</span>';
      }
      return '<span' + (info.cls ? ' class="' + info.cls + '"' : '') + (info.css ? ' style="' + info.css + '"' : '') + '>' + t.html + '</span>';
    }
    function lineHtml(li) {
      var h = lineCache[li];
      if (h === undefined) h = lineCache[li] = data.lines[li].map(runHtml).join('');
      return h;
    }
    function frameHtml(i) {
      var idxs = frames[i].l;
      if (crop) {
        return idxs.slice(crop[0], crop[1]).map(function (li) {
          return cropRuns(data.lines[li], crop[2], crop[3]).map(runHtml).join('');
        }).join('\n');
      }
      return idxs.map(lineHtml).join('\n');
    }

    /* ---- state ---- */
    var idx = 0;
    var want = !isStatic && truthy(root, 'autoplay');   // wants to play; a user pause clears it for good
    var loop = truthy(root, 'loop');
    var inView = !('IntersectionObserver' in window), docVisible = !document.hidden;
    var zoomed = false;
    var rm = rmQuery.matches;
    var rmCh = 0;
    var timer = 0, keyTimer = 0, tapTimer = 0, wasRunning = false;
    var shownKey = null, earlyTap = -1, earlyKey = -1, keyLeadT = 0, capLeadT = 0;
    var lastW = -1;

    function defaultPoster() {
      var want_ = root.getAttribute('data-poster'), c = -1;
      chapters.forEach(function (ch, i) { if (ch.id === want_ || String(i) === want_) c = i; });
      if (c < 0) chapters.forEach(function (ch, i) { if (c < 0 && ch.id === 'agents') c = i; });
      return Math.max(c, 0);
    }

    function running() { return want && !rm && inView && docVisible; }

    function capFor(i) {
      var c = chOf[i];
      for (var j = i; j >= 0 && chOf[j] === c; j--) if (frames[j].cap) return frames[j].cap;
      return (chapters[c] && chapters[c].cap) || '';
    }

    function setProgress(i, animate) {
      if (!progress) return;
      var bar_ = progress.firstChild, total = cum[n] || 1;
      bar_.style.transition = 'none';
      bar_.style.width = (cum[i] / total * 100) + '%';
      if (animate) {
        void bar_.offsetWidth;
        bar_.style.transition = 'width ' + (frames[i].d || 1000) + 'ms linear';
        bar_.style.width = (cum[i + 1] / total * 100) + '%';
      }
    }

    /* How long the chip stays for the run of consecutive frames sharing frame i's key. */
    function keyHold(i) {
      var k = frames[i].key, t = 0, j = i;
      while (j + 1 < n && frames[j + 1].key === k) { t += frames[j].d || 1000; j++; }
      return t + Math.min(frames[j].d || 1000, MAX_KEY_MS);
    }
    function keyReplacedNext(i) {   // the run ends on a frame that shows a different key: that frame swaps the chip, no hide
      var k = frames[i].key, j = i;
      while (j + 1 < n && frames[j + 1].key === k) j++;
      return !!(frames[j + 1] && frames[j + 1].key);
    }
    function hideKey() { keyTimer = 0; shownKey = null; keyChip.classList.remove('on'); }
    function ring(tap, live) {
      var r = el('div', 'pd-tap' + (live ? '' : ' pd-static'));
      r.style.setProperty('--col', tap[0]);
      r.style.setProperty('--row', tap[1]);
      return r;
    }

    /* ---- keycast overlay ---- */
    function kcAges() {
      kcGroups.forEach(function (g, k) { g.el.setAttribute('data-age', String(kcGroups.length - 1 - k)); });
    }
    /* FLIP: whatever shifts sideways when a cap is added or grows slides from where it was (the `translate` property, so it does
       not fight the age scale). `self` is the group that grew; it keeps its right edge and is not moved. */
    function kcFlip(change, self) {
      kcGroups.forEach(function (g) { g.x0 = g.el.offsetLeft; });
      change();
      kcGroups.forEach(function (g) {
        var d = g.x0 === undefined ? 0 : g.x0 - g.el.offsetLeft;
        g.x0 = undefined;
        if (g === self || rm || !d || !g.el.animate) return;
        g.el.animate([{ translate: d + 'px 0' }, { translate: '0 0' }], { duration: 160, easing: 'ease-out' });
      });
    }
    function kcPush(label, stack) {
      clearTimeout(kcClearT); kcClearT = 0;
      var last = kcGroups[kcGroups.length - 1];
      if (!stack) {   // a seek, a step or a loop: only this key
        if (last && kcGroups.length === 1 && last.label === label && last.n === 1 && kc.classList.contains('on')) return;
        kcGroups.forEach(function (g) { g.el.remove(); });
        kcGroups = []; last = null;
      }
      if (last && last.label === label) {       // the same key again: count it and pulse the cap
        kcFlip(function () {
          last.n++;
          last.el.innerHTML = capsHtml(label) + '<i>\u00d7' + last.n + '</i>';
        }, last);
        last.el.classList.remove('pulse'); void last.el.offsetWidth; last.el.classList.add('pulse');
        return;
      }
      if (last && isTyped(last.label) && isTyped(label) && label.length > last.label.length && label.indexOf(last.label) === 0) {
        kcFlip(function () {                    // typing: the cap grows in place
          last.label = label;
          last.el.innerHTML = capsHtml(label);
        }, last);
        return;
      }
      var g = { label: label, n: 1, el: el('span', 'pd-kc-g' + (isTyped(label) ? ' typed' : '')) };
      g.el.innerHTML = capsHtml(label);
      kcFlip(function () {
        kc.appendChild(g.el);
        kcGroups.push(g);
        while (kcGroups.length > KC_KEEP) { var old = kcGroups.shift(); old.el.classList.add('out'); (function (n) { setTimeout(function () { n.remove(); }, 250); })(old.el); }
      }, g);
      kcAges();
    }
    function kcHide() {
      kc.classList.remove('on');
      clearTimeout(kcClearT);
      kcClearT = setTimeout(function () { kcClearT = 0; kcGroups.forEach(function (g) { g.el.remove(); }); kcGroups = []; }, 300);
    }
    /* A key shows KEY_LEAD ms before the frame it produces: the cap is pushed by a timer near the end of the frame before it
       (earlyKey), and when that frame renders it is not pushed again. A frame reached by a seek or a loop pushes its own. */
    function renderKeys(i, viaAdvance, live) {
      var f = frames[i], nx = frames[i + 1];
      clearTimeout(keyTimer); keyTimer = 0;
      clearTimeout(keyLeadT); keyLeadT = 0;
      var early = live && earlyKey === i;
      earlyKey = -1;
      if (!f.key) { shownKey = null; kcHide(); }
      else {
        if (!early) kcPush(f.key, !!(viaAdvance && live && kc.classList.contains('on')));   // every frame with a key is a press
        shownKey = f.key;
        kc.classList.add('on');
        if (live && !(nx && nx.key)) keyTimer = setTimeout(function () { keyTimer = 0; shownKey = null; kcHide(); }, Math.min(f.d || 1000, MAX_KEY_MS));
      }
      if (live && nx && nx.key) {
        keyLeadT = setTimeout(function () {
          keyLeadT = 0;
          earlyKey = i + 1;
          kcPush(nx.key, kc.classList.contains('on'));
          shownKey = nx.key;
          kc.classList.add('on');
        }, Math.max(0, (f.d || 1000) - KEY_LEAD));
      }
    }
    /* during a loop dip: this frame's caption and key are already in place when the screen comes back */
    function preroll(i) {
      if (isStatic) return;
      if (cap) setCap(capFor(i));
      if (!overlay) return;
      if (frames[i].key) { kcPush(frames[i].key, false); shownKey = frames[i].key; kc.classList.add('on'); } else { shownKey = null; kcHide(); }
    }

    /* viaAdvance: frame i arrived by the playback timer (its tap ring may already have run). */
    function renderHud(i, viaAdvance) {
      if (isStatic) return;
      var f = frames[i], live = running(), nx = frames[i + 1];
      if (overlay) {
        renderKeys(i, viaAdvance, live);
      } else if (!keyChip) {
        // no frame of this recording shows a key
      } else if (f.key) {
        // consecutive frames with the same key keep one chip and one timer, so typing never flickers it
        if (!(viaAdvance && live && keyTimer && shownKey === f.key)) {
          clearTimeout(keyTimer); keyTimer = 0;
          if (shownKey !== f.key) { keyChip.innerHTML = keyHtml(f.key); shownKey = f.key; }
          keyChip.classList.add('on');
          if (live && !keyReplacedNext(i)) keyTimer = setTimeout(hideKey, keyHold(i));
        }
      } else {
        clearTimeout(keyTimer); keyTimer = 0;
        shownKey = null;
        keyChip.classList.remove('on');
      }
      clearTimeout(tapTimer); tapTimer = 0;
      var early = viaAdvance && live && earlyTap === i;
      earlyTap = -1;
      if (!early) {
        tapbox.innerHTML = '';
        // paused: mark the tap that comes next, as the moving version does; never the tap that produced this frame
        if (!live && nx && nx.tap && !crop) tapbox.appendChild(ring(nx.tap, false));
      }
      // the tap that produces frame i+1 happens before it: ring it during the end of this frame
      if (live && nx && nx.tap && !crop) {
        tapTimer = setTimeout(function () {
          tapTimer = 0;
          earlyTap = i + 1;
          tapbox.innerHTML = '';
          tapbox.appendChild(ring(nx.tap, true));
        }, Math.max(0, (f.d || 1000) - TAP_MS));
      }
      clearTimeout(capLeadT); capLeadT = 0;
      if (cap && live && nx && capFor(i + 1) !== capFor(i)) {   // the caption changes with the frame, not after it
        capLeadT = setTimeout(function () { capLeadT = 0; setCap(capFor(i + 1)); }, Math.max(0, (f.d || 1000) - CAP_LEAD));
      }
    }

    function syncUI() {
      if (!controls) return;
      var active = rm ? rmCh : chOf[idx];
      chapBtns.forEach(function (b, i) {
        if (i === active) b.setAttribute('aria-current', 'step'); else b.removeAttribute('aria-current');
      });
      playBtn.setAttribute('aria-pressed', want && !rm ? 'true' : 'false');
      if (rm && chapters.length) {
        prevBtn.disabled = rmCh <= 0;
        nextBtn.disabled = rmCh >= chapters.length - 1;
      } else {
        prevBtn.disabled = idx <= 0;
        nextBtn.disabled = idx >= n - 1;
      }
    }

    /* A new caption fades the old text out and itself in (CSS .pd-cap-out); with reduced motion it just swaps. */
    var capText = null, capTimer = 0;
    function setCap(c) {
      if (c === capText) return;
      capText = c;
      clearTimeout(capTimer);
      var t = cap.querySelector('.pd-cap-t');   // the text fades, not the caption's box
      if (!t) { cap.textContent = ''; t = el('span', 'pd-cap-t'); cap.appendChild(t); }
      if (rm || !t.textContent) { t.classList.remove('pd-cap-out'); t.textContent = c; return; }
      t.classList.add('pd-cap-out');
      capTimer = setTimeout(function () { capTimer = 0; t.textContent = capText; t.classList.remove('pd-cap-out'); }, 120);
    }

    function render(i, animateProgress, viaAdvance) {
      idx = Math.max(0, Math.min(n - 1, i));
      screen.innerHTML = frameHtml(idx);
      if (cap) setCap(capFor(idx));
      setProgress(idx, !!animateProgress);
      renderHud(idx, viaAdvance);
      syncUI();
    }

    function stopTimers() {
      clearTimeout(timer); timer = 0;
      clearTimeout(tapTimer); tapTimer = 0;
      clearTimeout(keyLeadT); keyLeadT = 0; earlyKey = -1;
      clearTimeout(capLeadT); capLeadT = 0;
      screen.classList.remove('pd-fading');
    }

    function schedule() {
      clearTimeout(timer);
      timer = setTimeout(advance, frames[idx].d || 1000);
      setProgress(idx, true);
    }

    function advance() {
      timer = 0;
      if (!running()) return;
      if (idx + 1 < n) {
        render(idx + 1, true, true);
        timer = setTimeout(advance, frames[idx].d || 1000);
      } else if (loop) {
        screen.classList.add('pd-fading');
        preroll(0);
        timer = setTimeout(function () {
          timer = 0;
          screen.classList.remove('pd-fading');
          if (!running()) return;
          render(0, true);
          timer = setTimeout(advance, frames[0].d || 1000);
        }, FADE_MS);
      } else {
        want = false;
        sync();
      }
    }

    /* Re-evaluate whether to run after any state change. */
    function sync() {
      var live = running();
      if (live) {
        if (!timer) schedule();
      } else {
        stopTimers();
        if (wasRunning) setProgress(idx, false);
        if (cap) setCap(capFor(idx));   // a caption that was changing ahead of its frame goes back
      }
      if (live !== wasRunning) { wasRunning = live; renderHud(idx); }
      syncUI();
    }

    /* ---- commands ---- */
    function toChapter(i) {
      i = Math.max(0, Math.min(chapters.length - 1, i));
      rmCh = i;
      render(!rm && running() ? (chapters[i].frame || 0) : posterOf(i));   // playing: from the start of the chapter; otherwise its poster
      rmCh = i;
      syncUI();
    }
    function jump(i) {   // chapter button: keeps playing if it was playing
      stopTimers();
      toChapter(i);
      wasRunning = false;
      sync();
    }
    function step(dir) {
      want = false;
      stopTimers();
      if (rm && chapters.length) toChapter(rmCh + dir);
      else render(idx + dir);
      sync();
    }
    function togglePlay() {
      if (want) { want = false; sync(); return; }
      if (!loop && idx >= n - 1) render(0);
      want = true;
      wasRunning = false;
      sync();
    }

    api.play = function () { if (!want) togglePlay(); };
    api.preroll = preroll;
    api.pause = function () { if (want) { want = false; sync(); } };
    api.seek = function (i) {
      want = false; stopTimers();
      render(i);
      sync();
    };
    api.chapter = function (i) {
      want = false; stopTimers();
      toChapter(i);
      sync();
    };

    /* ---- sizing ---- */
    function fit() {
      var avail = stage.clientWidth;
      if (!avail) return;
      var lim = FIT[isStatic ? 'static' : kind];
      var cs = getComputedStyle(term), ws = getComputedStyle(win);
      var overhead = parseFloat(cs.paddingLeft) + parseFloat(cs.paddingRight) +
        parseFloat(ws.borderLeftWidth) + parseFloat(ws.borderRightWidth) +
        parseFloat(ws.paddingLeft) + parseFloat(ws.paddingRight);
      var r = cellRatio(), ideal = (avail - overhead) / (cols * r);
      var fs, scale = 1, mode = '';
      if (zoomed) {
        fs = ZOOM_FS;   // the enlarged view: a readable size, the terminal scrolls inside it
      } else if (ideal >= lim[0]) {
        fs = Math.min(ideal, lim[1]);
      } else {
        fs = lim[0];
        scale = avail / (cols * r * fs + overhead);
        if (fs * scale < MIN_SCALED_FS && kind === 'desktop') { scale = 1; mode = 'scroll'; }
        else mode = 'scaled';
      }
      fs = Math.floor(fs * 4) / 4;
      // whole 1/64 px cells (what layout can represent) so glyph boxes and text never drift apart
      root.style.setProperty('--pd-cw', Math.floor(r * fs * 64) / 64 + 'px');
      root.style.setProperty('--pd-fs', fs + 'px');
      var lh = Math.round(fs * LINE_H), cwpx = Math.floor(r * fs * 64) / 64;
      root.style.setProperty('--pd-lh', lh + 'px');
      // box-drawing geometry in px: line thickness, line position in the cell, corner radius
      var bw = Math.max(1, Math.round(fs * 0.085)), bx = Math.floor((cwpx - bw) / 2), by = Math.floor((lh - bw) / 2);
      var cx = bx + bw / 2, br = Math.max(2, Math.min(Math.round(cwpx / 2), Math.floor(cx)));
      [['bw', bw], ['bx', bx], ['by', by], ['cx', cx], ['cy', by + bw / 2], ['br', br], ['s', br + bw]].forEach(function (v) {
        root.style.setProperty('--pd-' + v[0], v[1] + 'px');
      });
      root.classList.toggle('pd-small', !!zoomBtn && !zoomed && mode === 'scaled' && fs * scale < SMALL_FS);
      root.classList.toggle('pd-scaled', mode === 'scaled');
      root.classList.toggle('pd-scroll', mode === 'scroll');
      if (!isStatic) {
        if (mode === 'scroll') {
          scroller.setAttribute('tabindex', '0');
          scroller.setAttribute('role', 'region');
          scroller.setAttribute('aria-label', 'Desktop demo, scrolls sideways');
        } else {
          scroller.removeAttribute('tabindex'); scroller.removeAttribute('role'); scroller.removeAttribute('aria-label');
        }
      }
      win.style.transform = mode === 'scaled' ? 'scale(' + scale + ')' : '';
      stage.style.height = mode === 'scaled' ? Math.ceil(win.offsetHeight * scale) + 'px' : '';
      lastW = avail;
      updateLabel();
      updateMore();
    }

    function updateMore() {
      var more = root.classList.contains('pd-scroll') && scroller.scrollLeft + scroller.clientWidth < scroller.scrollWidth - 2;
      root.classList.toggle('pd-more', more);
    }

    function layout() { fit(); }

    /* ---- the enlarged view (a desktop demo scaled down on a narrow screen): the stage becomes a full-screen dialog,
       the terminal at ZOOM_FS inside it, scrolling both ways. Close button, Esc, and focus stays inside. ---- */
    function setZoom(on) {
      if (on === zoomed || !zoomBtn) return;
      zoomed = on;
      var de = document.documentElement;
      if (on) {
        root.style.minHeight = root.offsetHeight + 'px';   // the page behind does not jump
        stage.setAttribute('role', 'dialog'); stage.setAttribute('aria-modal', 'true'); stage.setAttribute('aria-label', 'paddock ' + what + ', enlarged');
        de.classList.add('pd-noscroll');
      } else {
        root.style.minHeight = '';
        stage.removeAttribute('role'); stage.removeAttribute('aria-modal'); stage.removeAttribute('aria-label');
        de.classList.remove('pd-noscroll');
      }
      root.classList.toggle('pd-zoomed', on);
      lastW = -1; fit();
      if (on) { stage.scrollLeft = 0; stage.scrollTop = 0; zoomClose.focus(); } else zoomBtn.focus();
    }
    api.zoom = function (on) { setZoom(on !== false); };

    function updateLabel() {
      if (isStatic) return;
      var scroll = root.classList.contains('pd-scroll');   // arrows scroll the demo there, so no stepping hint
      term.setAttribute('aria-label', 'paddock ' + what + (rm
        ? (scroll ? '' : ', press left and right arrow to step through chapters')
        : ', press space to play or pause'));
    }

    /* ---- mode (reduced motion) ---- */
    function applyMode(initial) {
      rm = rmQuery.matches;
      root.classList.toggle('pd-rm', rm && !isStatic);
      updateLabel();
      if (isStatic) return;
      stopTimers();
      if (rm) {
        if (!initial) rmCh = chapters.length ? chOf[idx] : 0;
        if (chapters.length) toChapter(rmCh);
      }
      wasRunning = false;
      sync();
    }

    /* ---- events ---- */
    if (!isStatic) {
      playBtn.addEventListener('click', togglePlay);
      prevBtn.addEventListener('click', function () { step(-1); });
      nextBtn.addEventListener('click', function () { step(1); });
      if (zoomBtn) {
        zoomBtn.addEventListener('click', function () { setZoom(true); });
        zoomClose.addEventListener('click', function () { setZoom(false); });
        stage.addEventListener('click', function (e) { if (root.classList.contains('pd-small') && !e.target.closest('button')) setZoom(true); });
        document.addEventListener('keydown', function (e) {
          if (!zoomed) return;
          if (e.key === 'Escape') { e.preventDefault(); setZoom(false); }
          else if (e.key === 'Tab') {   // keep focus in the dialog: close button, then the terminal
            var order = [zoomClose, term], at = order.indexOf(document.activeElement), to = order[(at + (e.shiftKey ? order.length - 1 : 1)) % order.length];
            e.preventDefault(); to.focus();
          }
        });
      }
      term.addEventListener('keydown', function (e) {
        if (e.target !== term || e.ctrlKey || e.metaKey || e.altKey) return;
        var k = e.key;
        if ((k === ' ' || k === 'k' || k === 'K') && !rm) { e.preventDefault(); togglePlay(); }
        else if (root.classList.contains('pd-scroll')) return;   // arrows keep scrolling the demo sideways
        else if (k === 'ArrowLeft') { e.preventDefault(); step(-1); }
        else if (k === 'ArrowRight') { e.preventDefault(); step(1); }
      });
      scroller.addEventListener('scroll', updateMore, { passive: true });
      document.addEventListener('visibilitychange', function () { docVisible = !document.hidden; sync(); });
      if ('IntersectionObserver' in window) {
        new IntersectionObserver(function (entries) {
          inView = entries[entries.length - 1].isIntersecting;
          sync();
        }, { threshold: 0.15 }).observe(root);
      }
      var onRm = function () { applyMode(false); };
      if (rmQuery.addEventListener) rmQuery.addEventListener('change', onRm); else rmQuery.addListener(onRm);
    }
    var refit = function () { if (stage.clientWidth !== lastW) fit(); };
    if ('ResizeObserver' in window) new ResizeObserver(refit).observe(root);
    else window.addEventListener('resize', refit);
    window.addEventListener('load', function () { ratio = 0; lastW = -1; refit(); });

    /* ---- start ---- */
    if (isStatic) {
      render(staticSpec ? Number(staticSpec[2]) : 0);
      layout();
    } else {
      rmCh = defaultPoster();
      if (rm) render(posterOf(rmCh)); else render(0);
      applyMode(true);
      layout();
    }
    root.setAttribute('data-pd-ready', '');
    api._resolve(api);
    return api;
  }

  function mount(target, opts) {
    var root = typeof target === 'string' ? document.getElementById(target) : target;
    if (!root) return null;
    var api = players.get(root);
    if (!api) { api = create(root, opts); players.set(root, api); }
    return api;
  }

  function get(target, opts) {
    return mount(target, opts);
  }

  window.PaddockPlayer = { mount: mount, get: get };

  function auto() {
    Array.prototype.forEach.call(document.querySelectorAll('.pd-player'), mount);
  }
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', auto); else auto();
})();
