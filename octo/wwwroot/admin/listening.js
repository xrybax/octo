// The Listening page: one person's plays on this server, or everyone's, drawn from what
// GET /api/admin/listening (the dashboard) or /api/family/.../listening (the Family page)
// answers. Both pages load this file and mount it into their own pane:
//
//   const page = OctoListening.mount(container, { fetch, cover, canSeeEveryone, title });
//
// fetch(days, user) answers { ok, status, data, error }; cover({ songId, coverId }, size) is a
// cover's address. days and user, when given, are the range and the person it opens on.
// restoreLights(), when given, puts the host page's own lights back; without it the lights go
// back to their defaults. The page returns { reload, pause, resume, destroy }: pause gives the
// page its own lights back (the Listening page lights it from the top artist's cover), resume
// takes them again and reloads.
(function () {
  'use strict';

  const RANGE_KEY = 'octo.listening.range';
  // The ranges every server offers; the answer's own `ranges` adds a year when plays are kept
  // that long.
  const RANGES = [7, 30, 90];
  const YEAR = 365;
  const rangeLabel = (n) => (n === YEAR ? '1 year' : `${n} days`);
  const DAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
  const GENRE_SHADES = [1, 0.78, 0.6, 0.46, 0.36, 0.28, 0.22];
  const LIGHTS = ['--amb-1', '--amb-2', '--amb-3', '--accent-rgb'];

  const ICON = {
    headphones: 'M201.89,54.66A103.43,103.43,0,0,0,128.79,24H128A104,104,0,0,0,24,128v56a24,24,0,0,0,24,24H64a24,24,0,0,0,24-24V144a24,24,0,0,0-24-24H40.36A88.12,88.12,0,0,1,190.54,65.93,87.39,87.39,0,0,1,215.65,120H192a24,24,0,0,0-24,24v40a24,24,0,0,0,24,24h24a24,24,0,0,0,24-24V128A103.41,103.41,0,0,0,201.89,54.66ZM64,136a8,8,0,0,1,8,8v40a8,8,0,0,1-8,8H48a8,8,0,0,1-8-8V136Zm160,48a8,8,0,0,1-8,8H192a8,8,0,0,1-8-8V144a8,8,0,0,1,8-8h24Z',
    clock: 'M128,24A104,104,0,1,0,232,128,104.11,104.11,0,0,0,128,24Zm0,192a88,88,0,1,1,88-88A88.1,88.1,0,0,1,128,216Zm64-88a8,8,0,0,1-8,8H128a8,8,0,0,1-8-8V72a8,8,0,0,1,16,0v48h48A8,8,0,0,1,192,128Z',
    fire: 'M183.89,153.34a57.6,57.6,0,0,1-46.56,46.55A8.75,8.75,0,0,1,136,200a8,8,0,0,1-1.32-15.89c16.57-2.79,30.63-16.85,33.44-33.45a8,8,0,0,1,15.78,2.68ZM216,144a88,88,0,0,1-176,0c0-27.92,11-56.47,32.66-84.85a8,8,0,0,1,11.93-.89l24.12,23.41,22-60.41a8,8,0,0,1,12.63-3.41C165.21,36,216,84.55,216,144Zm-16,0c0-46.09-35.79-85.92-58.21-106.33L119.52,98.74a8,8,0,0,1-13.09,3L80.06,76.16C64.09,99.21,56,122,56,144a72,72,0,0,0,144,0Z',
    heart: 'M178,40c-20.65,0-38.73,8.88-50,23.89C116.73,48.88,98.65,40,78,40a62.07,62.07,0,0,0-62,62c0,70,103.79,126.66,108.21,129a8,8,0,0,0,7.58,0C136.21,228.66,240,172,240,102A62.07,62.07,0,0,0,178,40ZM128,214.8C109.74,204.16,32,155.69,32,102A46.06,46.06,0,0,1,78,56c19.45,0,35.78,10.36,42.6,27a8,8,0,0,0,14.8,0c6.82-16.67,23.15-27,42.6-27a46.06,46.06,0,0,1,46,46C224,155.61,146.24,204.15,128,214.8Z',
    phone: 'M176,16H80A24,24,0,0,0,56,40V216a24,24,0,0,0,24,24h96a24,24,0,0,0,24-24V40A24,24,0,0,0,176,16Zm8,200a8,8,0,0,1-8,8H80a8,8,0,0,1-8-8V40a8,8,0,0,1,8-8h96a8,8,0,0,1,8,8ZM140,60a12,12,0,1,1-12-12A12,12,0,0,1,140,60Z',
    android: 'M176,148a12,12,0,1,1-12-12A12,12,0,0,1,176,148ZM92,136a12,12,0,1,0,12,12A12,12,0,0,0,92,136Zm148,24v24a16,16,0,0,1-16,16H32a16,16,0,0,1-16-16V161.13A113.38,113.38,0,0,1,51.4,78.72L26.34,53.66A8,8,0,0,1,37.66,42.34L63.82,68.5a111.43,111.43,0,0,1,128.55-.19l26-26a8,8,0,0,1,11.32,11.32L204.82,78.5c.75.71,1.5,1.43,2.24,2.17A111.25,111.25,0,0,1,240,160Zm-16,0a96,96,0,0,0-96-96h-.34C74.91,64.18,32,107.75,32,161.13V184H224Z',
    desktop: 'M208,40H48A24,24,0,0,0,24,64V176a24,24,0,0,0,24,24h72v16H96a8,8,0,0,0,0,16h64a8,8,0,0,0,0-16H136V200h72a24,24,0,0,0,24-24V64A24,24,0,0,0,208,40Zm8,136a8,8,0,0,1-8,8H48a8,8,0,0,1-8-8V64a8,8,0,0,1,8-8H208a8,8,0,0,1,8,8Z',
    radio: 'M104,168a8,8,0,0,1-8,8H64a8,8,0,0,1,0-16H96A8,8,0,0,1,104,168Zm-8-40H64a8,8,0,0,0,0,16H96a8,8,0,0,0,0-16Zm0-32H64a8,8,0,0,0,0,16H96a8,8,0,0,0,0-16ZM232,80V192a16,16,0,0,1-16,16H40a16,16,0,0,1-16-16V72a8,8,0,0,1,5.7-7.66l160-48a8,8,0,0,1,4.6,15.33L86.51,64H216A16,16,0,0,1,232,80ZM216,192V80H40V192H216Zm-16-56a40,40,0,1,1-40-40A40,40,0,0,1,200,136Zm-16,0a24,24,0,1,0-24,24A24,24,0,0,0,184,136Z',
    notes: 'M212.92,17.69a8,8,0,0,0-6.86-1.45l-128,32A8,8,0,0,0,72,56V166.08A36,36,0,1,0,88,196V110.25l112-28v51.83A36,36,0,1,0,216,164V24A8,8,0,0,0,212.92,17.69ZM52,216a20,20,0,1,1,20-20A20,20,0,0,1,52,216ZM88,93.75V62.25l112-28v31.5ZM180,184a20,20,0,1,1,20-20A20,20,0,0,1,180,184Z',
    warning: 'M236.8,188.09,149.35,36.22h0a24.76,24.76,0,0,0-42.7,0L19.2,188.09a23.51,23.51,0,0,0,0,23.72A24.35,24.35,0,0,0,40.55,224h174.9a24.35,24.35,0,0,0,21.33-12.19A23.51,23.51,0,0,0,236.8,188.09ZM222.93,203.8a8.5,8.5,0,0,1-7.48,4.2H40.55a8.5,8.5,0,0,1-7.48-4.2,7.59,7.59,0,0,1,0-7.72L120.52,44.21a8.75,8.75,0,0,1,15,0l87.45,151.87A7.59,7.59,0,0,1,222.93,203.8ZM120,144V104a8,8,0,0,1,16,0v40a8,8,0,0,1-16,0Zm20,36a12,12,0,1,1-12-12A12,12,0,0,1,140,180Z',
  };
  const svg = (name, cls) => `<svg${cls ? ` class="${cls}"` : ''} viewBox="0 0 256 256" aria-hidden="true" focusable="false"><path fill="currentColor" d="${ICON[name]}"/></svg>`;

  const reduced = () => window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  const esc = (v) => String(v ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const fmt1 = (n) => n.toLocaleString('en-US', { maximumFractionDigits: 1, minimumFractionDigits: n < 100 ? 1 : 0 });
  const fmt0 = (n) => Math.round(n).toLocaleString('en-US');
  const plural = (n, one, many) => `${fmt0(n)} ${Math.round(n) === 1 ? one : many}`;
  const dayOf = (iso) => new Date(`${iso}T12:00:00`);
  const shortDate = (d) => d.toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
  const hourLabel = (h) => (h === 0 ? '12 AM' : h === 12 ? '12 PM' : h < 12 ? `${h} AM` : `${h - 12} PM`);
  const minutesText = (m) => (m >= 60 ? `${Math.floor(m / 60)} h ${m % 60} min` : `${m} min`);

  function storedRange() {
    try {
      const n = Number(localStorage.getItem(RANGE_KEY));
      return RANGES.includes(n) || n === YEAR ? n : 30;
    } catch { return 30; }
  }
  function storeRange(n) {
    try { localStorage.setItem(RANGE_KEY, String(n)); } catch { /* storage off */ }
  }

  // ── Cover colors ───────────────────────────────────────────────────────────────────
  // The cover's strongest colors light the page, the way the dashboard's own lights do.
  function paletteOf(src) {
    return new Promise((resolve) => {
      if (!src) { resolve([]); return; }
      const img = new Image();
      img.crossOrigin = 'anonymous';
      img.onload = () => {
        try {
          const c = document.createElement('canvas');
          c.width = c.height = 32;
          const g = c.getContext('2d');
          g.drawImage(img, 0, 0, 32, 32);
          const px = g.getImageData(0, 0, 32, 32).data;
          const buckets = new Map();
          for (let i = 0; i < px.length; i += 4) {
            const r = px[i], gg = px[i + 1], b = px[i + 2];
            const max = Math.max(r, gg, b), min = Math.min(r, gg, b);
            const sat = max === 0 ? 0 : (max - min) / max;
            if (max < 40 || sat < 0.18) continue;
            const key = `${r >> 5},${gg >> 5},${b >> 5}`;
            const e = buckets.get(key) || { n: 0, r: 0, g: 0, b: 0, s: 0 };
            e.n++; e.r += r; e.g += gg; e.b += b; e.s += sat;
            buckets.set(key, e);
          }
          const ranked = [...buckets.values()]
            .map((e) => ({ r: e.r / e.n, g: e.g / e.n, b: e.b / e.n, score: Math.sqrt(e.n) * (0.3 + e.s / e.n) }))
            .sort((a, b) => b.score - a.score);
          resolve(ranked.slice(0, 3));
        } catch {
          // A cover from another site without leave to read it: the page keeps its lights.
          resolve([]);
        }
      };
      img.onerror = () => resolve([]);
      img.src = src;
    });
  }
  function toHsl({ r, g, b }) {
    r /= 255; g /= 255; b /= 255;
    const max = Math.max(r, g, b), min = Math.min(r, g, b), l = (max + min) / 2;
    if (max === min) return [0, 0, l];
    const d = max - min, s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
    const h = max === r ? (g - b) / d + (g < b ? 6 : 0) : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
    return [h * 60, s, l];
  }
  const hsl = (h, s, l) => `hsl(${Math.round(h)} ${Math.round(s * 100)}% ${Math.round(l * 100)}%)`;
  // A cover color as a light for the page: its hue kept, its strength held in a calm band.
  function soften(c, l) {
    const [h, s] = toHsl(c);
    return hsl(h, Math.min(0.7, Math.max(0.42, s)), l);
  }
  function accentOf(c) {
    const [h, s] = toHsl(c);
    const S = Math.min(0.62, Math.max(0.48, s)), L = 0.74;
    const k = (n) => (n + h / 30) % 12, a = S * Math.min(L, 1 - L);
    const f = (n) => Math.round(255 * (L - a * Math.max(-1, Math.min(k(n) - 3, 9 - k(n), 1))));
    return `${f(0)} ${f(8)} ${f(4)}`;
  }

  function niceMax(v) {
    for (const s of [0.5, 1, 2, 3, 4, 5, 6, 8, 10]) if (v <= s) return s;
    return Math.ceil(v / 2) * 2;
  }

  function mount(container, options) {
    const o = Object.assign({ canSeeEveryone: false, title: 'Listening' }, options || {});
    const uid = `ls${Math.random().toString(36).slice(2, 8)}`;
    const state = { days: RANGES.includes(o.days) || o.days === YEAR ? o.days : storedRange(), ranges: RANGES, who: o.user || null, data: null, people: [], layout: null, seq: 0, chart: 0, top: null, palette: null, paused: false };
    const shown = new Map();
    const style = document.documentElement.style;
    let alive = true;

    const root = document.createElement('div');
    root.className = 'listening';
    root.innerHTML = `
      <header class="page-header">
        <span class="page-icon" aria-hidden="true">${svg('headphones', 'icon')}</span>
        <div class="ls-words"><h1 tabindex="-1">${esc(o.title)}</h1><p class="ls-lede"></p></div>
        <div class="ls-controls">
          <div class="seg ls-who" role="group" aria-label="Whose listening" hidden><span class="seg-thumb"></span></div>
          <div class="seg ls-range" role="group" aria-label="Time range"><span class="seg-thumb"></span></div>
        </div>
      </header>
      <div class="ls-body"></div>`;
    container.replaceChildren(root);
    const $ = (sel) => root.querySelector(sel);
    const body = $('.ls-body');
    const rangeSeg = $('.ls-range');
    const whoSeg = $('.ls-who');
    // One button per range offered, after the switch's thumb.
    function drawRanges(list) {
      state.ranges = list;
      rangeSeg.querySelectorAll('button[data-days]').forEach((b) => b.remove());
      rangeSeg.insertAdjacentHTML('beforeend', list.map((n) => `<button type="button" data-days="${n}" aria-pressed="${n === state.days}">${rangeLabel(n)}</button>`).join(''));
    }
    drawRanges(RANGES);
    // Until the first answer says which ranges this server offers, the switch keeps its place
    // and shows nothing, so a year's button never pops in beside the others.
    rangeSeg.style.visibility = 'hidden';

    // ── Lights ─────────────────────────────────────────────────────────────────────────
    function applyPalette(p) {
      if (!p || !p.length) {
        if (typeof o.restoreLights === 'function') o.restoreLights();
        else LIGHTS.forEach((prop) => style.removeProperty(prop));
        return;
      }
      style.setProperty('--amb-1', soften(p[0], 0.36));
      style.setProperty('--amb-2', soften(p[1] || p[0], 0.32));
      style.setProperty('--amb-3', soften(p[2] || p[0], 0.30));
      style.setProperty('--accent-rgb', accentOf(p[0]));
    }
    async function light(src) {
      const p = await paletteOf(src);
      if (!alive || state.top !== src) return;
      state.palette = p;
      if (!state.paused) applyPalette(p);
    }

    // ── Switches ───────────────────────────────────────────────────────────────────────
    function placeThumb(seg) {
      const on = seg.querySelector('[aria-pressed="true"]');
      const thumb = seg.querySelector('.seg-thumb');
      if (!on || !on.offsetWidth) return;
      thumb.style.left = `${on.offsetLeft}px`;
      thumb.style.width = `${on.offsetWidth}px`;
    }
    const placeThumbs = () => { placeThumb(rangeSeg); if (!whoSeg.hidden) placeThumb(whoSeg); };

    rangeSeg.addEventListener('click', (e) => {
      const b = e.target.closest('button[data-days]');
      if (!b) return;
      const days = Number(b.dataset.days);
      if (days === state.days) return;
      state.days = days;
      storeRange(days);
      rangeSeg.querySelectorAll('button').forEach((x) => x.setAttribute('aria-pressed', String(Number(x.dataset.days) === days)));
      placeThumb(rangeSeg);
      load();
    });
    whoSeg.addEventListener('click', (e) => {
      const b = e.target.closest('button[data-who]');
      if (b) choose(b.dataset.who || null);
    });
    function choose(who) {
      if (who === state.who) return;
      state.who = who;
      renderWho();
      load();
    }
    function renderWho() {
      const list = o.canSeeEveryone
        ? (state.who ? [state.who] : state.people.length === 1 ? [state.people[0].name] : [])
        : [];
      whoSeg.hidden = !list.length;
      if (!list.length) return;
      whoSeg.innerHTML = `<span class="seg-thumb"></span>
        <button type="button" data-who="" aria-pressed="${!state.who}">Everyone</button>
        ${list.map((n) => `<button type="button" data-who="${esc(n)}" aria-pressed="${state.who === n}">${esc(n)}</button>`).join('')}`;
      placeThumb(whoSeg);
    }
    function renderHeading() {
      const own = !o.canSeeEveryone;
      $('h1').textContent = state.who || o.title;
      $('.ls-lede').textContent = state.who
        ? `${state.who}'s listening on this server.`
        : own ? 'Your listening on this server.' : 'What everyone on this server has been playing.';
    }

    // Numbers tween from what is shown to the new value.
    function tween(el, to, format, ms = 700) {
      const from = shown.get(el) ?? 0;
      shown.set(el, to);
      if (reduced() || from === to) { el.textContent = format(to); return; }
      const t0 = performance.now();
      const step = (t) => {
        const k = Math.min(1, (t - t0) / ms);
        const e = 1 - Math.pow(1 - k, 3);
        el.textContent = format(from + (to - from) * e);
        if (k < 1 && alive) requestAnimationFrame(step);
      };
      requestAnimationFrame(step);
    }

    // ── States ─────────────────────────────────────────────────────────────────────────
    // The chart's height for its width: shorter on a phone, where it is narrow.
    const chartHeight = (width) => (width < 480 ? 190 : 230);

    function skeleton() {
      state.layout = 'loading';
      const sk = (w, h, extra = '') => `<span class="ls-sk" style="width:${w};height:${h}px;${extra}"></span>`;
      const head = (title, sub) => `<h2>${esc(title)}</h2>${sub ? `<p class="ls-sub">${esc(sub)}</p>` : sk('300px', 14, 'margin:3px 0 19px')}`;
      const rowsSk = `<div class="ls-rows">${Array.from({ length: 6 }, () => `<div class="ls-sk-row">${sk('16px', 12)}${sk('48px', 48, 'border-radius:9px')}<div>${sk('60%', 12)}${sk('35%', 10, 'margin-top:8px')}</div></div>`).join('')}</div>`;
      const wide = body.clientWidth || 900;
      const chartWidth = (window.innerWidth <= 900 ? wide : (wide * 8) / 12) - 44;
      const heat = `<div class="ls-heat"><span></span>${Array.from({ length: 24 }, (_, h) => `<span class="ls-h">${h % 6 === 0 ? (h === 0 ? '12a' : h === 12 ? '12p' : h < 12 ? `${h}a` : `${h - 12}p`) : ''}</span>`).join('')}
        ${DAYS.map((d) => `<span class="ls-d">${d}</span>${'<span class="ls-c" style="--a:.04;animation:none"></span>'.repeat(24)}`).join('')}</div>
        <div class="ls-heat-foot">${sk('180px', 14)}</div>`;
      const people = o.canSeeEveryone && !state.who && state.people.length !== 1
        ? `<section class="ls-panel">${head('People', 'Everyone who listens through this server. Open a person to see their own page.')}
            <div class="ls-people">${[0, 1, 2].map(() => sk('100%', 88, 'border-radius:16px')).join('')}</div></section>`
        : '';
      body.innerHTML = `${people}
        <section class="ls-panel ls-hero" aria-busy="true" aria-label="Loading listening">
          <div>${sk('110px', 12)}${sk('220px', 88, 'margin:6px 0 4px;border-radius:14px')}${sk('240px', 22)}
            <div class="ls-facts">${sk('64px', 48)}${sk('80px', 48)}${sk('64px', 48)}${sk('90px', 48)}</div></div>
          <div class="ls-top">${sk('132px', 132, 'border-radius:16px')}<div>${sk('90px', 12)}${sk('120px', 22, 'margin-top:8px')}</div></div>
        </section>
        <div class="ls-grid">
          <section class="ls-panel ls-span-8">${head('Listening over time', 'Hours per day.')}${sk('100%', chartHeight(chartWidth), 'border-radius:12px')}</section>
          <section class="ls-panel ls-span-4">${head('Highlights', 'The moments that stood out.')}
            <div class="ls-highlights">${[86, 66, 86].map((h) => sk('100%', h, 'border-radius:16px')).join('')}</div></section>
          <section class="ls-panel ls-span-12">${head(o.canSeeEveryone ? 'When people listen' : 'When you listen')}${heat}</section>
          <section class="ls-panel ls-span-6">${head('Top artists', 'By plays.')}${rowsSk}</section>
          <section class="ls-panel ls-span-6">${head('Top songs', 'By plays.')}${rowsSk}</section>
        </div>`;
    }

    // A time as the server's clock shows it, the one the week grid and the days use: by the
    // zone's name when the browser knows it, else by its offset.
    function inServerTime(iso, d) {
      const t = new Date(iso);
      const date = { weekday: 'short', month: 'short', day: 'numeric' };
      const time = { hour: 'numeric', minute: '2-digit' };
      const show = (at, timeZone) => `${at.toLocaleDateString('en-US', { ...date, timeZone })}, ${at.toLocaleTimeString('en-US', { ...time, timeZone })}`;
      if (d.timeZoneName) {
        try { return show(t, d.timeZoneName); } catch { /* a zone this browser does not know */ }
      }
      return show(new Date(t.getTime() + (d.utcOffsetMinutes || 0) * 60000), 'UTC');
    }

    // The ranges offered: those the answer lists, none longer than the days kept.
    function fitRanges(d) {
      const offered = Array.isArray(d.ranges) && d.ranges.length ? d.ranges.map(Number) : RANGES;
      if (offered.join() !== state.ranges.join()) drawRanges(offered);
      const keep = Number(d.keepDays) || 90;
      const fits = offered.filter((n) => n <= keep);
      const allowed = fits.length ? fits : [offered[0]];
      rangeSeg.querySelectorAll('button[data-days]').forEach((b) => { b.hidden = !allowed.includes(Number(b.dataset.days)); });
      if (allowed.includes(state.days)) return false;
      state.days = allowed[allowed.length - 1];
      // The saved range follows, so the next visit asks for one the server keeps.
      storeRange(state.days);
      rangeSeg.querySelectorAll('button[data-days]').forEach((b) => b.setAttribute('aria-pressed', String(Number(b.dataset.days) === state.days)));
      return true;
    }

    // Refused outright (the dashboard sign-in is off, say): what to do instead, no retry.
    function renderLocked(message) {
      state.layout = 'locked';
      rangeSeg.hidden = true;
      body.innerHTML = `
        <section class="ls-panel ls-state ls-rise">
          <span class="ls-state-ic">${svg('headphones')}</span>
          <h2>Listening is hidden</h2>
          <p>${esc(message || 'Sign in to see listening.')}</p>
        </section>`;
      applyPalette(null);
      state.top = null;
    }

    function renderError(message) {
      state.layout = 'error';
      rangeSeg.style.visibility = '';
      body.innerHTML = `
        <section class="ls-panel ls-state ls-error ls-rise" role="alert">
          <span class="ls-state-ic">${svg('warning')}</span>
          <h2>Listening didn't load</h2>
          <p>${esc(message || 'Octo did not answer. Check the connection and try again.')}</p>
          <button type="button" class="btn ls-retry">Try again</button>
        </section>`;
      body.querySelector('.ls-retry').addEventListener('click', () => load());
    }

    function renderEmpty(d) {
      state.layout = 'empty';
      const r = d.rollup;
      const own = !o.canSeeEveryone;
      const someone = state.who;
      const onRecord = r.firstPlay != null;
      let title, text, action = '';
      if (onRecord) {
        title = `Nothing played in the last ${r.days} days`;
        text = `${someone ? `${someone}'s` : own ? 'Your' : 'Earlier'} listening is still here; a longer range shows it.`;
        const longer = state.ranges.find((n) => n > r.days && !rangeSeg.querySelector(`[data-days="${n}"]`).hidden);
        if (longer) action = `<button type="button" class="btn ls-longer" data-days="${longer}">Show ${longer === YEAR ? 'the last year' : `${longer} days`}</button>`;
      } else {
        title = someone ? `${someone} hasn't played anything yet` : own ? 'Your listening will show up here' : 'Listening will show up here';
        text = someone
          ? `Once ${someone} plays songs through this server, this page fills in.`
          : own
            ? 'Once you play songs through Octo, this page fills in with what you played and when.'
            : 'Once people play songs through this server, this page fills in with what everyone played and when.';
      }
      const note = d.recording
        ? `Plays are kept on this server for ${d.keepDays || 90} days.`
        : 'Recording is off on this server, so new plays are not kept.';
      body.innerHTML = `
        <section class="ls-panel ls-state ls-rise">
          <span class="ls-state-ic">${svg('headphones')}</span>
          <h2>${esc(title)}</h2>
          <p>${esc(text)}</p>
          ${onRecord ? '' : `<div class="ls-list"><span>Hours listened</span><span>Top artists and songs</span><span>Genres</span><span>Your week</span><span>Longest session</span><span>Apps and devices</span></div>`}
          ${action}
          <p class="ls-note">${esc(note)}</p>
        </section>`;
      const longer = body.querySelector('.ls-longer');
      if (longer) longer.addEventListener('click', () => rangeSeg.querySelector(`[data-days="${longer.dataset.days}"]`).click());
      rangeSeg.hidden = !onRecord;
      applyPalette(null);
      state.top = null;
    }

    function buildPage() {
      state.layout = 'page';
      shown.clear();
      state.top = null;
      body.innerHTML = `
        <section class="ls-panel ls-rise" style="--i:1" data-part="people" aria-labelledby="${uid}-people" hidden>
          <h2 id="${uid}-people">People</h2>
          <p class="ls-sub">Everyone who listens through this server. Open a person to see their own page.</p>
          <div class="ls-people"></div>
        </section>
        <section class="ls-panel ls-hero ls-rise" style="--i:2" aria-label="Summary">
          <div>
            <div class="ls-eyebrow" data-part="eyebrow"></div>
            <div class="ls-big"><span class="ls-num" data-part="hours">0</span><span class="ls-unit">hours</span></div>
            <div class="ls-delta" data-part="delta"></div>
            <div class="ls-facts">
              <div class="ls-fact"><span class="ls-v" data-part="plays">0</span><span class="ls-k">plays</span></div>
              <div class="ls-fact"><span class="ls-v" data-part="songs">0</span><span class="ls-k">different songs</span></div>
              <div class="ls-fact"><span class="ls-v" data-part="artists">0</span><span class="ls-k">artists</span></div>
              <div class="ls-fact"><span class="ls-v" data-part="library">0%</span><span class="ls-k" data-part="library-k"></span></div>
            </div>
          </div>
          <div class="ls-top">
            <span class="ls-cover"><img data-part="top-img" alt=""></span>
            <div class="ls-who"><span class="ls-k">Most played</span><span class="ls-n" data-part="top-name"></span><span class="ls-m" data-part="top-meta"></span></div>
          </div>
        </section>
        <div class="ls-grid">
          <section class="ls-panel ls-span-8 ls-rise" style="--i:3" aria-labelledby="${uid}-time">
            <h2 id="${uid}-time">Listening over time</h2>
            <p class="ls-sub">Hours per day.</p>
            <div class="ls-chart" data-part="chart"><div class="ls-tip"></div></div>
          </section>
          <section class="ls-panel ls-span-4 ls-rise" style="--i:4" aria-labelledby="${uid}-hl">
            <h2 id="${uid}-hl">Highlights</h2>
            <p class="ls-sub">The moments that stood out.</p>
            <div class="ls-highlights" data-part="highlights"></div>
          </section>
          <section class="ls-panel ls-span-12 ls-rise" style="--i:5" aria-labelledby="${uid}-heat">
            <h2 id="${uid}-heat">When ${o.canSeeEveryone ? 'people listen' : 'you listen'}</h2>
            <p class="ls-sub" data-part="heat-sub"></p>
            <div class="ls-heat" data-part="heat"></div>
            <div class="ls-heat-foot"><span data-part="peak"></span>
              <span class="ls-scale" aria-hidden="true">Less <i style="--a:.06"></i><i style="--a:.25"></i><i style="--a:.5"></i><i style="--a:.75"></i><i style="--a:1"></i> More</span></div>
          </section>
          <section class="ls-panel ls-span-6 ls-rise" style="--i:6" aria-labelledby="${uid}-ta">
            <h2 id="${uid}-ta">Top artists</h2>
            <p class="ls-sub">By plays.</p>
            <div class="ls-rows" data-part="artists-list"></div>
          </section>
          <section class="ls-panel ls-span-6 ls-rise" style="--i:7" aria-labelledby="${uid}-ts">
            <h2 id="${uid}-ts">Top songs</h2>
            <p class="ls-sub">By plays.</p>
            <div class="ls-rows" data-part="songs-list"></div>
          </section>
          <section class="ls-panel ls-span-7 ls-rise" style="--i:8" aria-labelledby="${uid}-g">
            <h2 id="${uid}-g">Genres</h2>
            <p class="ls-sub">Share of listening time.</p>
            <div class="ls-genres">
              <div class="ls-ring"><svg viewBox="0 0 168 168" data-part="ring" aria-hidden="true"></svg>
                <div class="ls-mid"><div><div class="ls-n" data-part="ring-n"></div><div class="ls-k" data-part="ring-k"></div></div></div></div>
              <div class="ls-legend" data-part="legend"></div>
            </div>
            <div class="ls-untagged" data-part="untagged"></div>
          </section>
          <section class="ls-panel ls-span-5 ls-rise" style="--i:9" aria-labelledby="${uid}-ap">
            <h2 id="${uid}-ap">Apps and devices</h2>
            <p class="ls-sub">Where the music played.</p>
            <div class="ls-apps" data-part="apps"></div>
          </section>
        </div>`;
    }
    const part = (name) => body.querySelector(`[data-part="${name}"]`);

    // ── Parts ──────────────────────────────────────────────────────────────────────────
    function renderPeople() {
      const panel = part('people');
      const show = o.canSeeEveryone && !state.who && state.people.length > 1;
      panel.hidden = !show;
      if (!show) return;
      part('people').querySelector('.ls-people').innerHTML = state.people.map((p, i) => {
        const days = p.perDay || [];
        const max = Math.max(...days, 0.01);
        const pts = days.map((h, j) => `${((j / Math.max(1, days.length - 1)) * 100).toFixed(2)},${(20 - (h / max) * 18).toFixed(2)}`).join(' L');
        const genre = p.topGenre ? ` · mostly ${esc(p.topGenre)}` : '';
        return `<button type="button" class="ls-person" data-person="${i}" aria-label="Open ${esc(p.name)}">
          <span class="ls-avatar" style="--hue:${[...(p.name || '')].reduce((a, c) => (a * 31 + c.charCodeAt(0)) % 360, 7)}" aria-hidden="true">${esc((p.name || '?').slice(0, 1).toUpperCase())}</span>
          <span class="ls-text"><span class="ls-n">${esc(p.name)}</span><span class="ls-s">${fmt1(p.hours)} h${genre}</span>
          ${pts ? `<svg viewBox="0 0 100 22" preserveAspectRatio="none" aria-hidden="true"><path d="M${pts}"/></svg>` : ''}</span></button>`;
      }).join('');
      panel.querySelectorAll('[data-person]').forEach((b) => b.addEventListener('click', () => {
        const p = state.people[Number(b.dataset.person)];
        if (p) choose(p.name);
      }));
    }

    function renderHero(d) {
      const r = d.rollup;
      part('eyebrow').textContent = `Last ${r.days} days`;
      tween(part('hours'), r.hours, fmt1);
      tween(part('plays'), r.plays, fmt0);
      tween(part('songs'), r.uniqueSongs, fmt0);
      tween(part('artists'), r.artists, fmt0);
      tween(part('library'), r.libraryShare * 100, (n) => `${Math.round(n)}%`);
      part('library-k').textContent = o.canSeeEveryone ? 'from the library' : 'from your library';
      // A change is only shown when the range before was on record for most of its length. A year
      // has no year before it on record (plays are kept a year at most), so it shows no change.
      const delta = part('delta');
      if (r.days >= YEAR) {
        const since = r.change.since || r.firstPlay;
        delta.textContent = since ? `Listening on record since ${shortDate(dayOf(since))}` : '';
      } else if (r.change.kind === 'change' && r.change.percent != null) {
        const pct = r.change.percent;
        delta.innerHTML = `<span class="ls-chip ${pct < 0 ? 'down' : ''}">${pct >= 0 ? '+' : '\u2212'}${Math.abs(pct)}%</span> vs the ${r.days} days before (${fmt1(r.hoursBefore)} h)`;
      } else {
        const since = r.change.since ? ` since ${shortDate(dayOf(r.change.since))}` : '';
        delta.innerHTML = `<span class="ls-chip none">New</span> Listening on record${since}`;
      }
      const top = r.topArtists[0];
      const img = part('top-img');
      const src = top ? o.cover({ songId: top.songId, coverId: top.coverId }, 264) : '';
      if (state.top !== src) {
        state.top = src;
        img.style.opacity = 0;
        setTimeout(() => {
          if (state.top !== src) return;
          img.onerror = () => { img.hidden = true; };
          img.hidden = !src;
          if (src) img.src = src; else img.removeAttribute('src');
          img.alt = top ? `${top.name} cover` : '';
          img.style.opacity = 1;
        }, reduced() ? 0 : 180);
        light(src);
      }
      part('top-name').textContent = top ? top.name : '';
      part('top-meta').textContent = top ? `${plural(top.plays, 'play', 'plays')} · ${fmt0(top.minutes)} min` : '';
    }

    let chartWidth = 0;
    function renderChart(r, still = reduced()) {
      const chart = part('chart');
      const W = Math.max(280, Math.round(chart.clientWidth) || 720), H = chartHeight(W), L = 34, R = 10, T = 12, B = 28;
      chartWidth = chart.clientWidth;
      const pts = r.perDay;
      const max = niceMax(Math.max(...pts.map((p) => p.hours), 0.1));
      const x = (i) => L + (i / Math.max(1, pts.length - 1)) * (W - L - R);
      const y = (h) => T + (1 - h / max) * (H - T - B);
      const ticks = [0, max / 2, max];
      // A soft curve through the days.
      let line = `M${x(0)},${y(pts[0].hours)}`;
      for (let i = 1; i < pts.length; i++) {
        const cx = (x(i - 1) + x(i)) / 2;
        line += ` C${cx},${y(pts[i - 1].hours)} ${cx},${y(pts[i].hours)} ${x(i)},${y(pts[i].hours)}`;
      }
      const area = `${line} L${x(pts.length - 1)},${y(0)} L${x(0)},${y(0)} Z`;
      // Labels counted back from today, so they are evenly spaced; the last one ends at the edge.
      const every = pts.length <= 7 ? (W < 480 ? 2 : 1) : pts.length <= 30 ? 7 : W < 480 ? 30 : 21;
      const last = pts.length - 1;
      const xl = pts.map((p, i) => ((last - i) % every === 0
        ? `<text x="${x(i)}" y="${H - 8}" text-anchor="${i === last ? 'end' : 'middle'}">${esc(shortDate(dayOf(p.day)))}</text>` : '')).join('');
      const peak = pts.reduce((a, p, i) => (p.hours > pts[a].hours ? i : a), 0);
      const id = `${uid}-${++state.chart}`;
      const html = `<svg viewBox="0 0 ${W} ${H}" role="img" aria-label="Hours listened per day, the most ${fmt1(pts[peak].hours)} hours on ${esc(shortDate(dayOf(pts[peak].day)))}">
        <defs><linearGradient id="fill-${id}" x1="0" x2="0" y1="0" y2="1"><stop offset="0" stop-color="rgb(var(--accent-rgb))" stop-opacity=".42"/><stop offset="1" stop-color="rgb(var(--accent-rgb))" stop-opacity="0"/></linearGradient>
        <clipPath id="draw-${id}"><rect x="0" y="0" height="${H}" width="${still ? W : 0}">${still ? '' : `<animate attributeName="width" from="0" to="${W}" dur="0.9s" fill="freeze" calcMode="spline" keyTimes="0;1" keySplines=".2 .8 .2 1"/>`}</rect></clipPath></defs>
        <g class="ls-tick">${ticks.map((t) => `<line x1="${L}" x2="${W - R}" y1="${y(t)}" y2="${y(t)}"/><text x="${L - 8}" y="${y(t) + 4}" text-anchor="end">${t % 1 ? t.toFixed(1) : t}h</text>`).join('')}${xl}</g>
        <g clip-path="url(#draw-${id})"><path d="${area}" fill="url(#fill-${id})"/><path class="ls-line" d="${line}"/>
        ${pts[peak].hours > 0 ? `<circle class="ls-dot" cx="${x(peak)}" cy="${y(pts[peak].hours)}" r="4"/>` : ''}</g>
        <line class="ls-hover-line" x1="0" x2="0" y1="${T}" y2="${H - B}" opacity="0"/>
        <rect class="ls-hit" x="${L}" y="0" width="${W - L - R}" height="${H}" fill="transparent"/></svg>`;
      chart.querySelector('svg')?.remove();
      chart.insertAdjacentHTML('afterbegin', html);
      const hit = chart.querySelector('.ls-hit'), hl = chart.querySelector('.ls-hover-line'), tip = chart.querySelector('.ls-tip');
      const move = (e) => {
        const box = chart.querySelector('svg').getBoundingClientRect();
        const sx = ((e.clientX - box.left) / box.width) * W;
        const i = Math.max(0, Math.min(pts.length - 1, Math.round(((sx - L) / (W - L - R)) * (pts.length - 1))));
        hl.setAttribute('x1', x(i)); hl.setAttribute('x2', x(i)); hl.setAttribute('opacity', 1);
        tip.style.left = `${(x(i) / W) * 100}%`;
        tip.style.top = `${(y(pts[i].hours) / H) * box.height}px`;
        tip.innerHTML = `${esc(dayOf(pts[i].day).toLocaleDateString('en-US', { weekday: 'short', month: 'short', day: 'numeric' }))} · <b>${fmt1(pts[i].hours)} h</b>`;
        tip.style.opacity = 1;
      };
      hit.addEventListener('pointermove', move);
      hit.addEventListener('pointerdown', move);
      hit.addEventListener('pointerleave', () => { tip.style.opacity = 0; hl.setAttribute('opacity', 0); });
    }

    function renderHighlights(d) {
      const r = d.rollup;
      const s = r.longestSession;
      const who = s && s.who ? ` · <span class="ls-keep">${esc(s.who)}</span>` : '';
      const when = s ? inServerTime(s.start, d) : '';
      const own = !o.canSeeEveryone;
      const heart = r.hearted[0];
      const top = r.topSongs[0];
      const isTop = heart && top && (heart.songId === top.songId || (heart.title === top.title && heart.artist === top.artist));
      const heartCover = heart ? o.cover({ songId: heart.songId, coverId: heart.coverId }, 80) : '';
      part('highlights').innerHTML = [
        s
          ? `<div class="ls-hl"><span class="ls-ic">${svg('clock')}</span><span><div class="ls-v">${minutesText(s.minutes)}</div><div class="ls-k">Longest session · <span class="ls-keep">${esc(when)}</span>${who}</div></span></div>`
          : `<div class="ls-hl"><span class="ls-ic">${svg('clock')}</span><span><div class="ls-v">No sessions yet</div><div class="ls-k">Plays less than 20 minutes apart make a session</div></span></div>`,
        `<div class="ls-hl"><span class="ls-ic">${svg('fire')}</span><span><div class="ls-v">${r.bestStreak === 1 ? '1 day' : `${r.bestStreak} days in a row`}</div><div class="ls-k">Best streak${r.currentStreak ? ` · ${r.currentStreak} now` : ''}</div></span></div>`,
        heart
          ? `<div class="ls-hl"><span class="ls-ic"><img src="${esc(heartCover)}" alt="" onerror="this.remove()"></span><span><div class="ls-v">${esc(heart.title)}</div><div class="ls-k">Hearted while playing · <span class="ls-keep">${esc(heart.artist)}${isTop ? `, now ${own ? 'your' : 'the'} top song` : ''}</span></div></span></div>`
          : `<div class="ls-hl"><span class="ls-ic">${svg('heart')}</span><span><div class="ls-v">No hearts in ${r.days} days</div><div class="ls-k">Songs ${own ? 'you heart' : 'hearted'} while they play show up here</div></span></div>`,
      ].join('');
    }

    function renderHeat(d) {
      const r = d.rollup;
      const max = Math.max(...r.grid.flat(), 1);
      const heat = part('heat');
      if (!heat.children.length) {
        let html = '<span></span>' + Array.from({ length: 24 }, (_, h) => `<span class="ls-h">${h % 6 === 0 ? (h === 0 ? '12a' : h === 12 ? '12p' : h < 12 ? `${h}a` : `${h - 12}p`) : ''}</span>`).join('');
        r.grid.forEach((row, ri) => {
          html += `<span class="ls-d">${DAYS[ri]}</span>` + row.map((_, h) => `<span class="ls-c" style="--r:${ri};--h:${h}" data-r="${ri}" data-h="${h}"></span>`).join('');
        });
        heat.innerHTML = html;
      }
      let best = [0, 0, 0];
      heat.querySelectorAll('.ls-c').forEach((c) => {
        const m = r.grid[c.dataset.r][c.dataset.h];
        if (m > best[0]) best = [m, Number(c.dataset.r), Number(c.dataset.h)];
        c.style.setProperty('--a', m ? (0.12 + 0.88 * Math.sqrt(m / max)).toFixed(3) : 0.04);
        c.title = `${DAYS[c.dataset.r]} ${hourLabel(Number(c.dataset.h))} · ${Math.round(m)} min`;
      });
      const tz = d.timeZone === 'UTC' || !d.timeZone ? 'in UTC' : `${d.timeZone} time`;
      part('heat-sub').textContent = `Minutes played in each hour of the week, ${tz}.`;
      part('peak').innerHTML = best[0] > 0
        ? `Busiest hour: <b>${DAYS[best[1]]}s around ${hourLabel(best[2])}</b>`
        : 'No busiest hour yet';
    }

    function rows(el, items, render) {
      el.innerHTML = items.map(render).join('');
      requestAnimationFrame(() => el.querySelectorAll('.ls-bar i').forEach((b) => { b.style.width = b.dataset.w; }));
    }
    const art = (item) => {
      const src = o.cover({ songId: item.songId, coverId: item.coverId }, 96);
      return `<span class="ls-art">${src ? `<img src="${esc(src)}" alt="" loading="lazy" onerror="this.remove()">` : ''}</span>`;
    };
    function renderTop(r) {
      const ma = r.topArtists[0] ? r.topArtists[0].plays : 1;
      rows(part('artists-list'), r.topArtists, (a, i) => `<div class="ls-row"><span class="ls-rank">${i + 1}</span>${art(a)}
        <div class="ls-t"><div class="ls-n">${esc(a.name)}</div><div class="ls-s">${fmt0(a.minutes)} min</div><div class="ls-bar"><i data-w="${(a.plays / ma) * 100}%"></i></div></div>
        <span class="ls-val">${plural(a.plays, 'play', 'plays')}</span></div>`);
      const ms = r.topSongs[0] ? r.topSongs[0].plays : 1;
      rows(part('songs-list'), r.topSongs, (s, i) => `<div class="ls-row"><span class="ls-rank">${i + 1}</span>${art(s)}
        <div class="ls-t"><div class="ls-n">${esc(s.title)}</div><div class="ls-s">${esc(s.artist)}${s.local ? '' : '<span class="ls-pill">Outside</span>'}</div><div class="ls-bar"><i data-w="${(s.plays / ms) * 100}%"></i></div></div>
        <span class="ls-val">${plural(s.plays, 'play', 'plays')}</span></div>`);
    }

    function renderGenres(r) {
      const total = r.genres.reduce((a, g) => a + g.hours, 0) || 1;
      const C = 2 * Math.PI * 66;
      const ring = part('ring');
      if (!ring.children.length) {
        ring.innerHTML = '<circle cx="84" cy="84" r="66" stroke="rgba(255,255,255,.06)"/>' +
          GENRE_SHADES.map((_, i) => `<circle cx="84" cy="84" r="66" data-seg="${i}" stroke-dasharray="0 ${C}" stroke-dashoffset="0"/>`).join('');
      }
      let off = 0;
      const placed = GENRE_SHADES.map((a, i) => {
        const g = r.genres[i];
        const len = g ? (g.hours / total) * C : 0;
        const at = off;
        off += len;
        return { a, len, at };
      });
      requestAnimationFrame(() => placed.forEach(({ a, len, at }, i) => {
        const seg = ring.querySelector(`[data-seg="${i}"]`);
        seg.setAttribute('stroke', `rgb(var(--accent-rgb) / ${a})`);
        const gap = len > 4 ? 2.5 : 0;
        seg.style.strokeDasharray = `${Math.max(0, len - gap)} ${C}`;
        seg.style.strokeDashoffset = `${-at}`;
      }));
      const top = r.genres[0];
      part('ring-n').textContent = top ? top.name : 'None yet';
      part('ring-k').textContent = top ? `${Math.round((top.hours / total) * 100)}% of the time` : '';
      part('legend').innerHTML = r.genres.map((g, i) => `<div class="ls-li"><span class="ls-sw" style="background:rgb(var(--accent-rgb) / ${GENRE_SHADES[i]})"></span><span>${esc(g.name)}</span><span class="ls-v">${fmt1(g.hours)} h</span></div>`).join('');
      part('untagged').textContent = r.untaggedHours >= 0.05
        ? `${fmt1(r.untaggedHours)} h of songs have no genre at all, from the song, its album or its artist, so they aren't counted here.`
        : '';
    }

    function renderApps(r) {
      const iconFor = (a) => (a.name === 'Internet radio' ? 'radio' : a.platform === 'iphone' ? 'phone' : a.platform === 'android' ? 'android'
        : ['windows', 'macos', 'linux'].includes(a.platform) ? 'desktop' : 'notes');
      rows(part('apps'), r.apps, (a) => `<div class="ls-app"><span class="ls-ic">${svg(iconFor(a))}</span>
        <div><div class="ls-n">${esc(a.name)}</div><div class="ls-bar"><i data-w="${a.share * 100}%"></i></div></div>
        <span class="ls-val">${a.share < 0.01 ? '<1' : Math.round(a.share * 100)}%</span></div>`);
    }

    function render() {
      const d = state.data;
      if (Array.isArray(d.people)) state.people = d.people;
      renderHeading();
      renderWho();
      if (fitRanges(d)) { load(); return; }
      rangeSeg.style.visibility = '';
      if (!d.rollup || d.rollup.plays === 0) { renderEmpty(d); placeThumbs(); return; }
      rangeSeg.hidden = false;
      if (state.layout !== 'page') buildPage();
      renderPeople();
      renderHero(d);
      renderChart(d.rollup);
      renderHighlights(d);
      renderHeat(d);
      renderTop(d.rollup);
      renderGenres(d.rollup);
      renderApps(d.rollup);
      placeThumbs();
    }

    async function load() {
      const mine = ++state.seq;
      if (state.layout !== 'page') skeleton();
      root.setAttribute('aria-busy', 'true');
      let reply;
      try {
        reply = await o.fetch(state.days, state.who);
      } catch (e) {
        reply = { ok: false, error: e && e.message };
      }
      if (!alive || mine !== state.seq) return;
      root.removeAttribute('aria-busy');
      if (reply && reply.status === 403) {
        renderLocked(reply.error);
        return;
      }
      if (!reply || !reply.ok || !reply.data || !reply.data.rollup) {
        renderError(reply && reply.error);
        return;
      }
      state.data = reply.data;
      render();
    }

    const onResize = () => {
      placeThumbs();
      const chart = state.layout === 'page' && state.data && part('chart');
      if (chart && Math.abs(chart.clientWidth - chartWidth) > 8) renderChart(state.data.rollup, true);
    };
    window.addEventListener('resize', onResize);
    if (document.fonts && document.fonts.ready) document.fonts.ready.then(() => alive && placeThumbs());
    renderHeading();
    placeThumbs();
    load();

    return {
      reload: () => load(),
      // Leaving the page: the page's own lights come back.
      pause() {
        state.paused = true;
        applyPalette(null);
      },
      resume() {
        state.paused = false;
        if (state.layout === 'page' && state.palette) applyPalette(state.palette);
        placeThumbs();
        load();
      },
      destroy() {
        alive = false;
        window.removeEventListener('resize', onResize);
        applyPalette(null);
      },
    };
  }

  window.OctoListening = { mount };
})();
