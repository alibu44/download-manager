const $ = (id) => document.getElementById(id);
const send = (m) => new Promise((res) => chrome.runtime.sendMessage(m, (r) => res(r || { ok: false, error: 'no reply' })));
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
const fmtSize = (b) => {
  if (!b) return '';
  const u = ['B', 'KB', 'MB', 'GB']; let i = 0;
  while (b >= 1024 && i < 3) { b /= 1024; i++; }
  return (i ? b.toFixed(1) : b) + ' ' + u[i];
};
const hint = (t) => { $('hint').textContent = t; };
const DOWNLOAD_URL = 'https://github.com/alibu44/download-manager/releases/latest';

(async () => {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  const r = await send({ type: 'status' });
  const alive = !!r.alive;
  $('enabled').checked = r.enabled !== false;
  $('dot').className = 'dot ' + (alive ? 'on' : 'off');
  $('state').textContent = alive ? 'Uygulama çalışıyor' : 'Uygulama kapalı';
  $('show').disabled = !alive;
  $('page').disabled = !alive || !/^https?:/i.test(tab?.url || '');
  if (!alive) {
    // The extension is only the browser half: the downloads are done by the desktop app.
    $('hint').innerHTML = 'Uygulama çalışmıyor ya da kurulu değil; indirmeler tarayıcıda normal şekilde yapılır. '
      + `Windows uygulamasını <a href="${DOWNLOAD_URL}" target="_blank" rel="noopener">buradan indirin</a>.`;
  } else if (!r.paired) {
    hint('Download Manager penceresinde bu eklentiye izin vermeniz gerekiyor (bir kez sorulur).');
  } else {
    hint('Tarayıcıda başlayan indirmeler önce uygulamada onayınıza sunulur. Videoların üzerinde "Bu videoyu indir" düğmesi çıkar.');
  }

  const media = (await send({ type: 'media', tabId: tab?.id })).data || [];
  $('mediaHead').hidden = !media.length;
  $('media').innerHTML = media.map((m, i) => `<button data-i="${i}" title="${esc(m.url)}"><span>${esc(m.name || m.type)}</span><span class="s">${m.kind === 'stream' ? esc(m.type) : fmtSize(m.size)}</span></button>`).join('');
  $('media').querySelectorAll('button').forEach((b) => b.addEventListener('click', async () => {
    const res = await send({ type: 'grab', item: media[b.dataset.i], tab: { url: tab.url, title: tab.title } });
    hint(res.ok ? 'İndirme başladı.' : 'Hata: ' + res.error);
  }));

  $('page').addEventListener('click', async () => {
    hint('Video kaliteleri alınıyor…');
    const res = await send({ type: 'formats', url: tab.url });
    if (!res.ok) return hint('Bu sayfada indirilebilir video bulunamadı: ' + res.error);
    const info = res.data;
    hint(info.title);
    $('formats').innerHTML = info.options.map((o, i) => `<button data-i="${i}"><span>${esc(o.label)}</span><span class="s">${o.size ? '~' + fmtSize(o.size) : ''}</span></button>`).join('');
    $('formats').querySelectorAll('button').forEach((b) => b.addEventListener('click', async () => {
      const o = info.options[b.dataset.i];
      const a = await send({ type: 'addVideo', url: info.webpage_url || tab.url, title: info.title, format: o, referer: tab.url });
      hint(a.ok ? 'İndirme başladı: ' + o.label : 'Hata: ' + a.error);
    }));
  });
})();

$('enabled').addEventListener('change', (e) => send({ type: 'setEnabled', enabled: e.target.checked }));
$('show').addEventListener('click', () => send({ type: 'show' }).then(() => window.close()));
