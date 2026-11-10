// Floating "Bu videoyu indir" button over the page's video, like IDM's download panel.
(() => {
  if (window.__dmLoaded) return;
  window.__dmLoaded = true;

  const YT = /(^|\.)youtube\.com$/.test(location.hostname);
  // The Chrome Web Store and Edge Add-ons do not accept extensions that download from YouTube;
  // tools/paketle.ps1 sets this to true for those two packages.
  const NO_YOUTUBE = false;
  if (NO_YOUTUBE && YT) return;
  const cache = new Map(); // videoUrl -> formats
  let host; let root; let currentUrl = '';

  function videoUrl() {
    if (!YT) return mainVideo() ? location.href : '';
    const u = new URL(location.href);
    if (u.pathname === '/watch' && u.searchParams.get('v')) return `https://www.youtube.com/watch?v=${u.searchParams.get('v')}`;
    const m = u.pathname.match(/^\/shorts\/([\w-]{6,})/);
    return m ? `https://www.youtube.com/shorts/${m[1]}` : '';
  }

  // The largest on-screen video that is real content (not a short looping background or ad tile).
  function mainVideo() {
    let best = null; let area = 0;
    for (const v of document.querySelectorAll('video')) {
      const r = v.getBoundingClientRect();
      if (r.width < 320 || r.height < 180 || r.bottom <= 0 || r.top >= innerHeight) continue;
      if (!v.currentSrc && !v.querySelector('source')) continue;
      if (isFinite(v.duration) && v.duration < 20) continue;
      if (r.width * r.height > area) { area = r.width * r.height; best = v; }
    }
    return best;
  }

  const fmtSize = (b) => {
    if (!b) return '';
    const u = ['B', 'KB', 'MB', 'GB']; let i = 0;
    while (b >= 1024 && i < 3) { b /= 1024; i++; }
    return '~' + (i ? b.toFixed(1) : b) + ' ' + u[i];
  };
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  const send = (m) => new Promise((res) => {
    try { chrome.runtime.sendMessage(m, (r) => res(r || { ok: false, error: chrome.runtime.lastError?.message || 'no reply' })); } catch (e) { res({ ok: false, error: String(e) }); }
  });

  const CSS = `
    :host { all: initial; }
    .wrap { position: fixed; z-index: 2147483000; font: 13px/1.35 "Segoe UI", Roboto, Arial, sans-serif; display:flex; flex-direction:column; align-items:flex-end; }
    .btn { display:inline-flex; align-items:center; gap:7px; padding:7px 12px 7px 9px; border:1px solid #2b7a33; border-radius:6px; cursor:pointer;
      background:linear-gradient(#4cbc56,#2f9a3a); color:#fff; font:600 13px "Segoe UI", Roboto, Arial, sans-serif; box-shadow:0 2px 10px rgb(0 0 0 / .35); opacity:.92; }
    .btn:hover { opacity:1; filter:brightness(1.05); }
    .btn svg { width:18px; height:18px; }
    .x { margin-left:4px; opacity:.8; font-weight:400; padding:0 2px; }
    .x:hover { opacity:1; }
    .panel { margin-top:6px; width:270px; max-height:60vh; overflow:auto; background:#fff; color:#1b1b1b; border:1px solid #9aa3b0; border-radius:6px; box-shadow:0 6px 24px rgb(0 0 0 / .35); }
    .head { padding:8px 10px; background:#f0f3f7; border-bottom:1px solid #dde1e8; font-weight:600; white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
    .opt { display:flex; justify-content:space-between; gap:8px; width:100%; padding:8px 12px; border:0; border-bottom:1px solid #eef0f3; background:#fff; color:inherit; font:inherit; text-align:left; cursor:pointer; }
    .opt:hover { background:#e5f3ff; }
    .opt span:first-child { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
    .opt .s { color:#6b7385; flex:none; }
    .note { padding:10px 12px; color:#5c6470; }
    .err { padding:10px 12px; color:#c62828; }
    .spin { display:inline-block; width:14px; height:14px; border:2px solid #cfd5dd; border-top-color:#2f6fde; border-radius:50%; animation:s .8s linear infinite; vertical-align:-2px; margin-right:6px; }
    @keyframes s { to { transform:rotate(360deg); } }
    .toast { margin-top:6px; padding:8px 12px; background:#1f2a37; color:#fff; border-radius:6px; box-shadow:0 4px 14px rgb(0 0 0 / .35); }
    @media (prefers-color-scheme: dark) {
      .panel { background:#23262b; color:#e8e8e8; border-color:#444a52; }
      .head { background:#2c3036; border-color:#3a3f46; }
      .opt { background:#23262b; border-color:#30343a; }
      .opt:hover { background:#26496f; }
      .opt .s, .note { color:#a2a9b4; }
    }`;

  const ICON = '<svg viewBox="0 0 24 24"><path d="M12 3v11m-5-5 5 5 5-5M5 19h14" stroke="#fff" stroke-width="2.4" fill="none" stroke-linecap="round" stroke-linejoin="round"/></svg>';

  function mount() {
    host = document.createElement('div');
    host.id = 'dm-download-host';
    root = host.attachShadow({ mode: 'open' }); // shadow DOM keeps the page's CSS out of our UI
    root.innerHTML = `<style>${CSS}</style><div class="wrap" id="w">
      <div class="btn" id="b" role="button" tabindex="0" title="Download Manager ile indir">${ICON}<span>Bu videoyu indir</span><span class="x" id="x" title="Gizle">✕</span></div>
      <div id="p"></div></div>`;
    document.documentElement.appendChild(host);
    root.getElementById('b').addEventListener('click', (e) => {
      if (e.target.id === 'x') { host.style.display = 'none'; return; }
      togglePanel();
    });
    document.addEventListener('click', (e) => { if (!e.composedPath().includes(host)) root.getElementById('p').innerHTML = ''; }, true);
  }

  // Keep the button at the top-right corner of the video player.
  function place() {
    const url = videoUrl();
    if (!url) { if (host) host.style.display = 'none'; return; }
    if (!host) mount();
    // A new video brings the button back even if it was hidden with ✕.
    if (url !== currentUrl) { currentUrl = url; root.getElementById('p').innerHTML = ''; host.style.display = ''; }
    const player = !YT ? mainVideo() : document.querySelector(url.includes('/shorts/')
      ? 'ytd-reel-video-renderer[is-active] #player-container, #shorts-player'
      : '#movie_player');
    const w = root.getElementById('w');
    const r = player?.getBoundingClientRect();
    if (r && r.width > 200 && r.bottom > 0 && r.top < innerHeight) {
      w.style.top = Math.max(r.top + 12, 8) + 'px';
      w.style.bottom = '';
      w.style.right = Math.max(innerWidth - r.right + 12, 8) + 'px';
    } else {
      w.style.top = ''; w.style.right = '16px'; w.style.bottom = '16px';
    }
  }

  function toast(p, r, label) {
    p.innerHTML = r.ok ? `<div class="toast">İndirme başladı: ${esc(label)}</div>` : `<div class="panel"><div class="err">${esc(r.error)}</div></div>`;
    setTimeout(() => { if (p.querySelector('.toast')) p.innerHTML = ''; }, 3500);
  }

  // yt-dlp does not know this site: offer the media files/streams the page itself loaded.
  async function showSniffed(p, error) {
    const r = await send({ type: 'media' });
    const list = (r.ok && r.data) || [];
    if (!list.length) {
      p.innerHTML = `<div class="panel"><div class="err">Video bilgisi alınamadı: ${esc(error)}</div></div>`;
      return;
    }
    p.innerHTML = `<div class="panel"><div class="head">Bu sayfada yakalanan medya</div>
      ${list.map((m, i) => `<button class="opt" data-i="${i}"><span title="${esc(m.url)}">${esc(m.name || m.type)}</span><span class="s">${m.kind === 'stream' ? esc(m.type) : fmtSize(m.size)}</span></button>`).join('')}</div>`;
    p.querySelectorAll('.opt').forEach((b) => b.addEventListener('click', async () => {
      const m = list[b.dataset.i];
      p.innerHTML = '<div class="toast"><span class="spin"></span>Gönderiliyor…</div>';
      toast(p, await send({ type: 'grab', item: m, tab: { url: location.href, title: document.title } }), m.name || m.type);
    }));
  }

  async function togglePanel() {
    const p = root.getElementById('p');
    if (p.innerHTML) { p.innerHTML = ''; return; }
    const url = currentUrl;
    p.innerHTML = '<div class="panel"><div class="note"><span class="spin"></span>Video kaliteleri alınıyor…</div></div>';
    let info = cache.get(url);
    if (!info) {
      const r = await send({ type: 'formats', url });
      if (!r.ok) {
        if (r.error === 'app-offline') {
          p.innerHTML = '<div class="panel"><div class="err">Download Manager çalışmıyor. Uygulamayı açıp tekrar deneyin.</div></div>';
          return;
        }
        return showSniffed(p, r.error);
      }
      info = r.data; cache.set(url, info);
    }
    if (url !== currentUrl) return;
    p.innerHTML = `<div class="panel"><div class="head" title="${esc(info.title)}">${esc(info.title)}</div>
      ${info.options.map((o, i) => `<button class="opt" data-i="${i}"><span>${esc(o.label)}</span><span class="s">${fmtSize(o.size)}</span></button>`).join('')}</div>`;
    p.querySelectorAll('.opt').forEach((b) => b.addEventListener('click', async () => {
      const o = info.options[b.dataset.i];
      p.innerHTML = '<div class="toast"><span class="spin"></span>Gönderiliyor…</div>';
      toast(p, await send({ type: 'addVideo', url: info.webpage_url || url, title: info.title, format: o, referer: YT ? '' : location.href }), o.label);
    }));
  }

  // Single-page apps: re-check on navigation, on scroll/resize, and periodically.
  document.addEventListener('yt-navigate-finish', place);
  addEventListener('scroll', place, { passive: true });
  addEventListener('resize', place);
  setInterval(place, 1000);
  place();
})();
