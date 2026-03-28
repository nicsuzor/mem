import { chromium } from 'playwright';
import { mkdirSync } from 'fs';

const URL = 'http://100.115.15.120:5173';
const OUT = '/Users/suzor/src/mem/overwhelm-dashboard/qa/screenshots/review-2026-03-27';
mkdirSync(OUT, { recursive: true });

async function run() {
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });

  await page.goto(URL, { waitUntil: 'networkidle', timeout: 30000 });
  await page.waitForTimeout(3000);

  // Find the actual scroll container
  const scrollInfo = await page.evaluate(() => {
    // Check if the main scrollable element is not the window
    const candidates = document.querySelectorAll('[class*="overflow"], section, main, [class*="scroll"]');
    const scrollable = [];
    for (const el of candidates) {
      if (el.scrollHeight > el.clientHeight + 10) {
        scrollable.push({
          tag: el.tagName,
          class: el.className?.slice(0, 120),
          scrollHeight: el.scrollHeight,
          clientHeight: el.clientHeight,
          overflow: getComputedStyle(el).overflowY
        });
      }
    }
    // Also check body/html
    scrollable.push({
      tag: 'BODY',
      scrollHeight: document.body.scrollHeight,
      clientHeight: document.body.clientHeight,
      overflow: getComputedStyle(document.body).overflowY
    });
    scrollable.push({
      tag: 'HTML',
      scrollHeight: document.documentElement.scrollHeight,
      clientHeight: document.documentElement.clientHeight,
      overflow: getComputedStyle(document.documentElement).overflowY
    });
    return scrollable;
  });
  console.log('Scroll containers:', JSON.stringify(scrollInfo, null, 2));

  // Find the real scrollable container and scroll it
  const scrollContainer = await page.evaluate(() => {
    const candidates = document.querySelectorAll('[class*="overflow"]');
    for (const el of candidates) {
      if (el.scrollHeight > el.clientHeight + 100) {
        return { 
          selector: el.tagName + '.' + el.className?.split(' ').join('.').slice(0, 200),
          scrollHeight: el.scrollHeight,
          clientHeight: el.clientHeight
        };
      }
    }
    return null;
  });
  console.log('Primary scroll container:', JSON.stringify(scrollContainer));

  // Scroll the overflow container
  if (scrollContainer) {
    for (let i = 1; i <= 12; i++) {
      await page.evaluate((n) => {
        const candidates = document.querySelectorAll('[class*="overflow"]');
        for (const el of candidates) {
          if (el.scrollHeight > el.clientHeight + 100) {
            el.scrollTop = el.scrollHeight * (n / 12);
            break;
          }
        }
      }, i);
      await page.waitForTimeout(300);
      await page.screenshot({ path: `${OUT}/40-realscroll-${String(i).padStart(2,'0')}.png`, fullPage: false });
      console.log(`Scroll ${i}/12`);
    }
  }

  // Also try Quick Capture button click
  console.log('\n=== Quick Capture float test ===');
  const floatBtn = await page.locator('button.fixed').first();
  if (await floatBtn.count() > 0) {
    console.log('Found fixed button');
    await floatBtn.click({ force: true });
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${OUT}/50-quickcapture-open.png`, fullPage: false });
    // close
    await page.keyboard.press('Escape');
    await page.waitForTimeout(300);
  } else {
    console.log('No fixed button found');
    // Try Alt+C
    await page.keyboard.press('Alt+c');
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${OUT}/50-quickcapture-altc.png`, fullPage: false });
  }

  await browser.close();
  console.log('Done');
}

run().catch(e => { console.error(e); process.exit(1); });
