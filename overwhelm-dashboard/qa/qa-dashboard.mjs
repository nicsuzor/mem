import { chromium } from 'playwright';
import { mkdirSync } from 'fs';

const URL = 'http://100.115.15.120:5173';
const OUT = '/Users/suzor/src/mem/overwhelm-dashboard/qa/screenshots';
mkdirSync(OUT, { recursive: true });

async function run() {
  const browser = await chromium.launch({ headless: true });
  
  // Desktop viewport
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  
  console.log('Navigating to dashboard...');
  await page.goto(URL, { waitUntil: 'networkidle', timeout: 30000 });
  await page.waitForTimeout(2000); // let animations settle
  
  // 1. Full landing view
  console.log('Screenshot: landing view');
  await page.screenshot({ path: `${OUT}/01-landing.png`, fullPage: false });
  
  // 2. Full page screenshot
  console.log('Screenshot: full page');
  await page.screenshot({ path: `${OUT}/02-full-page.png`, fullPage: true });
  
  // 3. PathTimeline section - look for it
  console.log('Looking for PathTimeline...');
  const pathTimeline = await page.locator('text=Path Timeline, text=PathTimeline, text=path-timeline, [class*="timeline"], [class*="path"]').first();
  if (await pathTimeline.count() > 0) {
    await pathTimeline.scrollIntoViewIfNeeded();
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${OUT}/03-path-timeline.png`, fullPage: false });
  } else {
    console.log('PathTimeline not found by text, taking viewport screenshot after scroll');
  }
  
  // 4. Scroll to different sections and capture
  // Scroll down incrementally
  for (let i = 1; i <= 6; i++) {
    await page.evaluate((n) => window.scrollBy(0, window.innerHeight * 0.8), i);
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${OUT}/04-scroll-${i}.png`, fullPage: false });
    console.log(`Screenshot: scroll position ${i}`);
  }
  
  // 5. Look for Quick Capture floating button
  console.log('Looking for Quick Capture button...');
  const quickCapture = await page.locator('[class*="capture"], [class*="quick-capture"], button:has-text("Capture"), button:has-text("capture"), [class*="floating"]').first();
  if (await quickCapture.count() > 0) {
    await quickCapture.screenshot({ path: `${OUT}/05-quick-capture-btn.png` });
    console.log('Found Quick Capture button, clicking...');
    await quickCapture.click();
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${OUT}/05-quick-capture-open.png`, fullPage: false });
  } else {
    console.log('Quick Capture button not found');
    // Try to find any fixed/floating element at bottom-right
    const floaters = await page.evaluate(() => {
      const els = document.querySelectorAll('button, [role="button"]');
      const fixed = [];
      for (const el of els) {
        const style = window.getComputedStyle(el);
        if (style.position === 'fixed' || style.position === 'sticky') {
          fixed.push({ tag: el.tagName, text: el.textContent?.trim()?.slice(0, 50), classes: el.className });
        }
      }
      return fixed;
    });
    console.log('Fixed/floating elements:', JSON.stringify(floaters, null, 2));
  }
  
  // 6. Check for crew session consolidation
  console.log('Checking for crew entries...');
  const crewText = await page.evaluate(() => {
    const body = document.body.innerText;
    const lines = body.split('\n').filter(l => l.toLowerCase().includes('crew'));
    return lines;
  });
  console.log('Crew-related text:', JSON.stringify(crewText));
  
  // 7. Check for "All quiet" empty state
  console.log('Checking for empty state...');
  const allQuiet = await page.evaluate(() => {
    return document.body.innerText.includes('All quiet') || document.body.innerText.includes('all quiet');
  });
  console.log('All quiet found:', allQuiet);
  
  // 8. Check active sessions  
  const sessionText = await page.evaluate(() => {
    const body = document.body.innerText;
    const lines = body.split('\n').filter(l => l.toLowerCase().includes('session') || l.toLowerCase().includes('active'));
    return lines.slice(0, 20);
  });
  console.log('Session-related text:', JSON.stringify(sessionText));
  
  // 9. Look for Today's Story section
  console.log("Looking for Today's Story...");
  const storySection = await page.locator("text=Today's Story, text=Today, [class*='story']").first();
  if (await storySection.count() > 0) {
    await storySection.scrollIntoViewIfNeeded();
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${OUT}/06-todays-story.png`, fullPage: false });
  }
  
  // 10. Back to top, then get all section headers
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.waitForTimeout(300);
  const headers = await page.evaluate(() => {
    const hs = document.querySelectorAll('h1, h2, h3, h4, h5, h6, [class*="heading"], [class*="title"], [class*="section"]');
    return Array.from(hs).map(h => ({ tag: h.tagName, text: h.textContent?.trim()?.slice(0, 80), classes: h.className?.slice(0, 80) }));
  });
  console.log('Section headers:', JSON.stringify(headers, null, 2));
  
  // 11. Check grid/layout classes for responsive verification
  const gridInfo = await page.evaluate(() => {
    const grids = document.querySelectorAll('[class*="grid"], [class*="Grid"]');
    return Array.from(grids).map(g => ({ classes: g.className?.slice(0, 120), children: g.children.length }));
  });
  console.log('Grid layouts:', JSON.stringify(gridInfo, null, 2));
  
  // 12. Mobile viewport
  console.log('Switching to mobile viewport...');
  await page.setViewportSize({ width: 375, height: 812 });
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.waitForTimeout(1000);
  await page.screenshot({ path: `${OUT}/07-mobile-landing.png`, fullPage: false });
  await page.screenshot({ path: `${OUT}/07-mobile-full.png`, fullPage: true });
  
  // 13. Check for PathTimeline expand/collapse
  console.log('Checking PathTimeline expand/collapse...');
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.waitForTimeout(500);
  
  // Look for show-more or expand buttons
  const expandBtns = await page.locator('button:has-text("show"), button:has-text("more"), button:has-text("expand"), [class*="show-more"], [class*="expand"]');
  const expandCount = await expandBtns.count();
  console.log(`Found ${expandCount} expand/show-more buttons`);
  for (let i = 0; i < Math.min(expandCount, 5); i++) {
    const text = await expandBtns.nth(i).textContent();
    console.log(`  Button ${i}: "${text?.trim()}"`);
  }
  
  // Try clicking a timeline group to expand
  const timelineGroups = await page.locator('[class*="timeline"] [class*="group"], [class*="timeline"] [class*="project"], [class*="path"] [class*="group"]');
  const groupCount = await timelineGroups.count();
  console.log(`Found ${groupCount} timeline groups`);
  
  // 14. Look at the page structure more carefully
  const pageStructure = await page.evaluate(() => {
    const root = document.querySelector('main, #app, [class*="app"], body > div');
    if (!root) return 'No root found';
    const walk = (el, depth) => {
      if (depth > 3) return '';
      const tag = el.tagName?.toLowerCase();
      const cls = el.className?.toString()?.slice(0, 60) || '';
      const txt = el.children.length === 0 ? el.textContent?.trim()?.slice(0, 40) : '';
      let result = '  '.repeat(depth) + `<${tag} class="${cls}">${txt}\n`;
      for (const child of el.children) {
        result += walk(child, depth + 1);
      }
      return result;
    };
    return walk(root, 0);
  });
  console.log('Page structure (first 3000 chars):\n', pageStructure.slice(0, 3000));
  
  await browser.close();
  console.log('\nDone! Screenshots saved to', OUT);
}

run().catch(e => { console.error(e); process.exit(1); });
