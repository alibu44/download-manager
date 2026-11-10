// End-to-end check: extension in a throwaway Edge profile + the running DownloadManager.exe.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import puppeteer from 'puppeteer-core';

const EXT = path.resolve('../extension');
const API = 'http://127.0.0.1:52931/api/';
const EDGE = 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe';
const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'dm-edge-'));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const list = async () => (await (await fetch(API + 'list')).json()).items;
const log = (...a) => console.log('•', ...a);

const browser = await puppeteer.launch({
  executablePath: EDGE, headless: false, pipe: true, enableExtensions: [EXT], userDataDir: profile,
  args: ['--no-first-run', '--no-default-browser-check', '--window-size=1280,900'], defaultViewport: null,
});
try {
  const sw = await browser.waitForTarget((t) => t.type() === 'service_worker' && t.url().startsWith('chrome-extension://'), { timeout: 15000 });
  log('extension loaded:', new URL(sw.url()).host);

  // 1) A normal file download is taken over by the app.
  const fileUrl = 'https://www.python.org/ftp/python/3.13.0/python-3.13.0-embed-amd64.zip';
  const before = (await list()).length;
  const page = await browser.newPage();
  await page.goto('https://www.python.org/downloads/release/python-3130/', { waitUntil: 'domcontentloaded' });
  await page.evaluate((u) => { const a = document.createElement('a'); a.href = u; document.body.append(a); a.click(); }, fileUrl);
  let item;
  for (let i = 0; i < 40 && !item; i++) { await sleep(500); item = (await list()).find((d) => d.url === fileUrl); }
  log('caught by app:', !!item, item && `${item.fileName} referer=${item.referer}`);
  for (let i = 0; i < 60 && item && item.status !== 'complete' && item.status !== 'error'; i++) { await sleep(1000); item = (await list()).find((d) => d.url === fileUrl); }
  log('app download status:', item?.status, item?.size, item?.error || '');
  log('items before/after:', before, (await list()).length);

  // 2) YouTube button -> quality list -> start download.
  await page.goto('https://www.youtube.com/watch?v=jNQXAC9IVRw', { waitUntil: 'domcontentloaded' });
  if (page.url().includes('consent.')) {
    log('consent page shown, rejecting non-essential cookies');
    await page.evaluate(() => [...document.querySelectorAll('button')].find((b) => /reject|reddet/i.test(b.textContent))?.click());
    await page.waitForNavigation({ waitUntil: 'domcontentloaded' }).catch(() => {});
  }
  await page.waitForFunction(() => document.querySelector('#dm-download-host')?.shadowRoot?.getElementById('b'), { timeout: 20000 });
  await sleep(1500);
  const pos = await page.evaluate(() => {
    const r = document.querySelector('#dm-download-host').shadowRoot.getElementById('b').getBoundingClientRect();
    const p = document.querySelector('#movie_player').getBoundingClientRect();
    return { btn: [Math.round(r.left), Math.round(r.top), Math.round(r.width)], player: [Math.round(p.left), Math.round(p.top), Math.round(p.right)] };
  });
  log('button at', pos.btn, 'player', pos.player);
  await page.screenshot({ path: 'yt-button.png' });
  await page.evaluate(() => document.querySelector('#dm-download-host').shadowRoot.getElementById('b').click());
  await page.waitForFunction(() => document.querySelector('#dm-download-host').shadowRoot.querySelectorAll('.opt').length > 0, { timeout: 60000 });
  const opts = await page.evaluate(() => [...document.querySelector('#dm-download-host').shadowRoot.querySelectorAll('.opt')].map((b) => b.innerText.replace(/\s+/g, ' ')));
  log('qualities:', opts.join(' | '));
  await page.screenshot({ path: 'yt-panel.png' });
  const nBefore = (await list()).length;
  await page.evaluate(() => document.querySelector('#dm-download-host').shadowRoot.querySelector('.opt').click());
  await page.waitForFunction(() => /İndirme başladı/.test(document.querySelector('#dm-download-host').shadowRoot.getElementById('p').innerText), { timeout: 15000 });
  log('toast:', await page.evaluate(() => document.querySelector('#dm-download-host').shadowRoot.getElementById('p').innerText));
  log('app items grew:', nBefore, '->', (await list()).length);
} finally {
  await browser.close();
  fs.rmSync(profile, { recursive: true, force: true });
}
