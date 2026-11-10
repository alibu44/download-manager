// Hands browser downloads to the Download Manager app (http://127.0.0.1:52931),
// the way IDM's browser integration takes over downloads, and keeps a per-tab list
// of the video/audio files and HLS/DASH streams each page loads.
const API = 'http://127.0.0.1:52931/api/';
const passThrough = new Set(); // URLs we deliberately let the browser download
const SMALL = 1024 * 1024; // files below this finish faster in the browser than a hand-off takes
// Set to true by tools/paketle.ps1 for the Chrome Web Store / Edge Add-ons packages (store policy).
const NO_YOUTUBE = false;

async function getEnabled() {
  const { enabled = true } = await chrome.storage.local.get('enabled');
  return enabled;
}

async function appAlive() {
  try {
    const r = await fetch(API + 'ping', { signal: AbortSignal.timeout(800) });
    return (await r.json()).app === 'download-manager';
  } catch {
    return false;
  }
}

// Store builds and Firefox have an ID the app does not know in advance: the app asks the user
// once ("allow this browser extension?") and remembers the answer. Returns whether we are allowed.
async function paired() {
  try {
    const r = await fetch(API + 'pair', { method: 'POST', signal: AbortSignal.timeout(1500) });
    return r.ok && (await r.json()).paired === true;
  } catch {
    return false;
  }
}

async function post(path, body) {
  const r = await fetch(API + path, {
    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body),
  });
  const j = await r.json().catch(() => ({}));
  if (r.status === 403) {
    await paired(); // makes the app show its pairing dialog
    throw new Error('Download Manager penceresinde bu eklentiye izin verin, sonra tekrar deneyin.');
  }
  if (!r.ok) throw new Error(j.error || `HTTP ${r.status}`);
  return j;
}

async function cookieHeader(url) {
  try {
    const cs = await chrome.cookies.getAll({ url });
    return cs.map((c) => `${c.name}=${c.value}`).join('; ');
  } catch {
    return '';
  }
}

// ask: the app shows its "download file info" dialog (name, folder, start / later / cancel) first.
async function sendToApp(url, { filename = '', referer = '', show = true, ask = false, size = 0 } = {}) {
  return post('add', {
    url, filename, referer, show, ask, size,
    cookies: await cookieHeader(url),
    userAgent: navigator.userAgent,
  });
}

const baseName = (p) => (p || '').split(/[\\/]/).pop();

chrome.downloads.onCreated.addListener(async (item) => {
  const url = item.finalUrl || item.url;
  if (!/^https?:/i.test(url) || item.byExtensionId === chrome.runtime.id) return;
  if (passThrough.delete(url)) return;
  if (item.state !== 'in_progress') return; // restored history entry or already finished
  if (item.totalBytes > 0 && item.totalBytes < SMALL) return;
  if (!(await getEnabled()) || !(await appAlive())) return; // app closed: let the browser download
  if (!(await paired())) return; // not allowed by the app yet: the browser keeps its download

  try {
    await chrome.downloads.cancel(item.id);
    await chrome.downloads.erase({ id: item.id });
  } catch { return; /* finished in the meantime: the browser already has the file */ }
  try {
    await sendToApp(url, { filename: baseName(item.filename), referer: item.referrer || '', ask: true, size: Math.max(item.totalBytes, 0) });
  } catch {
    // The app refused it: fall back to a normal browser download.
    passThrough.add(url);
    chrome.downloads.download({ url });
  }
});

// Right-click a link -> "Download Manager ile indir"
chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create({ id: 'dm-link', title: 'Download Manager ile indir', contexts: ['link', 'video', 'audio', 'image'] });
  });
});
chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  const url = info.linkUrl || info.srcUrl;
  if (!url || !/^https?:/i.test(url) || !(await appAlive())) return;
  sendToApp(url, { referer: tab?.url || '', ask: true }).catch(() => {});
});

/* ---------- media sniffer ---------- */
const MEDIA_EXT = /\.(mp4|mkv|webm|mov|avi|flv|m4v|mp3|m4a|flac|ogg|opus|wav|aac)$/i;
const STREAM_EXT = /\.(m3u8|mpd)$/i;
const SEGMENT = /\.(ts|m4s|cmfv|cmfa)$|(^|[\/_-])(seg|segment|frag|chunk)[-_]?\d+/i;
const MAX_PER_TAB = 30;
let chain = Promise.resolve(); // serialises storage updates from concurrent requests

const key = (tabId) => 'm' + tabId;
async function mediaFor(tabId) {
  return (await chrome.storage.session.get(key(tabId)))[key(tabId)] || [];
}
function setBadge(tabId, n) {
  chrome.action.setBadgeBackgroundColor({ tabId, color: '#2e9d3a' }).catch(() => {});
  chrome.action.setBadgeText({ tabId, text: n ? String(n) : '' }).catch(() => {});
}
function remember(tabId, entry) {
  chain = chain.then(async () => {
    const list = await mediaFor(tabId);
    if (list.some((m) => m.url === entry.url) || list.length >= MAX_PER_TAB) return;
    list.push(entry);
    await chrome.storage.session.set({ [key(tabId)]: list });
    setBadge(tabId, list.length);
  }).catch(() => {});
}

chrome.webRequest.onHeadersReceived.addListener((d) => {
  if (d.tabId < 0 || (d.statusCode !== 200 && d.statusCode !== 206)) return;
  let u;
  try { u = new URL(d.url); } catch { return; }
  if (/(^|\.)googlevideo\.com$/.test(u.hostname)) return; // YouTube: handled by the video button
  const h = Object.fromEntries((d.responseHeaders || []).map((x) => [x.name.toLowerCase(), x.value || '']));
  const type = (h['content-type'] || '').toLowerCase();
  const size = +((h['content-range'] || '').split('/')[1] || h['content-length'] || 0) || 0;
  let name = u.pathname.split('/').pop() || '';
  try { name = decodeURIComponent(name); } catch { /* keep the raw name */ }
  if (STREAM_EXT.test(u.pathname) || /mpegurl|dash\+xml/.test(type)) {
    return remember(d.tabId, { url: d.url, kind: 'stream', type: /mpd|dash/.test(type + u.pathname) ? 'DASH' : 'HLS', size: 0, name });
  }
  // Only whole files a <video>/<audio> element loads; MSE players fetch fragments via XHR instead.
  if (d.type !== 'media' || SEGMENT.test(u.pathname)) return;
  if (!/^(video|audio)\//.test(type) && !MEDIA_EXT.test(u.pathname)) return;
  if (size && size < SMALL) return;
  remember(d.tabId, { url: d.url, kind: 'file', type: type.split(';')[0] || 'media', size, name });
}, { urls: ['http://*/*', 'https://*/*'], types: ['media', 'xmlhttprequest', 'other', 'object'] }, ['responseHeaders']);

chrome.tabs.onUpdated.addListener((tabId, change) => {
  if (change.status === 'loading' && change.url) {
    chain = chain.then(() => chrome.storage.session.remove(key(tabId))).catch(() => {});
    setBadge(tabId, 0);
  }
});
chrome.tabs.onRemoved.addListener((tabId) => { chrome.storage.session.remove(key(tabId)); });

async function grab(m, tab) {
  if (m.kind === 'stream') {
    return post('add', {
      url: m.url, title: tab.title || m.name || 'video', description: `Akış (${m.type})`, referer: tab.url || '',
      format: { label: `Akış (${m.type})`, format: 'bv*+ba/b', merge: 'mp4' }, userAgent: navigator.userAgent,
    });
  }
  return sendToApp(m.url, { referer: tab.url || '', show: false });
}

// Messages from the video button and the popup.
chrome.runtime.onMessage.addListener((msg, sender, reply) => {
  (async () => {
    try {
      if (msg.type === 'status') {
        const alive = await appAlive();
        return reply({ ok: true, alive, enabled: await getEnabled(), paired: alive && await paired() });
      }
      if (msg.type === 'setEnabled') { await chrome.storage.local.set({ enabled: !!msg.enabled }); return reply({ ok: true }); }
      if (msg.type === 'media') return reply({ ok: true, data: await mediaFor(msg.tabId ?? sender.tab?.id) });
      if (!(await appAlive())) return reply({ ok: false, error: 'app-offline' });
      if (msg.type === 'formats') {
        if (NO_YOUTUBE && /^https?:\/\/([^/]+\.)?(youtube\.com|youtu\.be)\//i.test(msg.url)) {
          return reply({ ok: false, error: 'Mağaza sürümü YouTube videolarını indirmez (mağaza kuralı).' });
        }
        return reply({ ok: true, data: await post('formats', { url: msg.url }) });
      }
      if (msg.type === 'addVideo') {
        return reply({ ok: true, data: await post('add', { url: msg.url, title: msg.title, description: msg.format.label, format: msg.format, referer: msg.referer || '', userAgent: navigator.userAgent }) });
      }
      if (msg.type === 'grab') return reply({ ok: true, data: await grab(msg.item, msg.tab || sender.tab || {}) });
      if (msg.type === 'show') return reply({ ok: true, data: await post('show', {}) });
      reply({ ok: false, error: 'unknown' });
    } catch (e) {
      reply({ ok: false, error: String(e.message || e) });
    }
  })();
  return true; // async reply
});
