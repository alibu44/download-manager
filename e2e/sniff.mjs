// Checks the media sniffer and the generic video button on a non-YouTube page.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import puppeteer from 'puppeteer-core';

const EXT = path.resolve('../extension');
const EDGE = 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe';
const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'dm-edge-'));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const browser = await puppeteer.launch({
  executablePath: EDGE, headless: false, pipe: true, enableExtensions: [EXT], userDataDir: profile,
  args: ['--no-first-run', '--no-default-browser-check', '--window-size=1280,900', '--autoplay-policy=no-user-gesture-required'], defaultViewport: null,
});
try {
  const swTarget = await browser.waitForTarget((t) => t.type() === 'service_worker' && t.url().startsWith('chrome-extension://'), { timeout: 15000 });
  const sw = await swTarget.worker();
  const page = await browser.newPage();
  for (const url of ['https://hls-js.netlify.app/demo/', 'https://commons.wikimedia.org/wiki/File:Big_Buck_Bunny_4K.webm']) {
    await page.goto(url, { waitUntil: 'domcontentloaded' });
    await sleep(4000);
    await page.evaluate(() => { document.querySelector('video')?.play().catch(() => {}); });
    await sleep(6000);
    const media = await sw.evaluate(async () => Object.values(await chrome.storage.session.get(null)).flat());
    console.log(url, '\n  sniffed:', media.map((m) => `${m.kind}/${m.type} ${m.name} ${m.size}`).join(' | ') || '(none)');
    const btn = await page.evaluate(() => !!document.getElementById('dm-download-host') && document.getElementById('dm-download-host').style.display !== 'none');
    console.log('  video button shown:', btn);
  }
} finally {
  await browser.close();
  fs.rmSync(profile, { recursive: true, force: true });
}
