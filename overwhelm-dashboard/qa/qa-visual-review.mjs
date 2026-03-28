import { chromium } from 'playwright';
import { mkdirSync } from 'fs';

const URL = 'http://100.115.15.120:5173';
const OUT = '/Users/suzor/src/mem/overwhelm-dashboard/qa/screenshots/review-2026-03-27';
mkdirSync(OUT, { recursive: true });

async function run() {
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });

  console.log('--- Navigating to dashboard ---');
  await page.goto(URL, { waitUntil: 'networkidle', timeout: 30000 });
  await page.waitForTimeout(3000);

  // 1. Landing view
  console.log('=== 01: Landing view ===');
  await page.screenshot({ path: `${OUT}/01-landing.png`, fullPage: false });

  // 2. Full page
  console.log('=== 02: Full page ===');
  await page.screenshot({ path: `${OUT}/02-full-page.png`, fullPage: true });

  // 3. Dump all visible text and section headers
  const allHeaders = await page.evaluate(() => {
    const hs = document.querySelectorAll('h1, h2, h3, h4, h5, h6');
    return Array.from(hs).map(h => `${h.tagName}: ${h.textContent?.trim()?.slice(0, 100)}`);
  });
  console.log('All section headers:', JSON.stringify(allHeaders, null, 2));

  // 4. Check PathTimeline - count project groups
  console.log('\n=== CHECK 1: PathTimeline ===');
  const pathInfo = await page.evaluate(() => {
    // Look for PathTimeline by scanning component text
    const all = document.body.innerText;
    const pathIdx = all.indexOf('Path');
    const timelineIdx = all.indexOf('Timeline');
    // Look for any section containing "path" or "timeline"
    const sections = document.querySelectorAll('section, [class*="timeline"], [class*="path"], [class*="Path"]');
    const results = [];
    for (const s of sections) {
      results.push({ tag: s.tagName, class: s.className?.slice(0, 80), textPreview: s.textContent?.trim()?.slice(0, 200) });
    }
    return { pathIdx, timelineIdx, sections: results };
  });
  console.log('PathTimeline scan:', JSON.stringify(pathInfo, null, 2));

  // Look for "show more" or expand buttons
  const expandButtons = await page.evaluate(() => {
    const buttons = document.querySelectorAll('button');
    return Array.from(buttons).map(b => ({
      text: b.textContent?.trim()?.slice(0, 60),
      class: b.className?.slice(0, 80),
      visible: b.offsetParent !== null
    })).filter(b => b.text && b.visible);
  });
  console.log('All visible buttons:', JSON.stringify(expandButtons, null, 2));

  // 5. Check Today's Story section
  console.log("\n=== CHECK 2: Today's Story ===");
  const storyInfo = await page.evaluate(() => {
    const body = document.body.innerText;
    const storyIdx = body.indexOf("Today's Story");
    if (storyIdx < 0) return { found: false };
    const excerpt = body.slice(storyIdx, storyIdx + 800);
    return { found: true, excerpt };
  });
  console.log("Today's Story:", JSON.stringify(storyInfo, null, 2));

  // Find and screenshot Today's Story
  const storyEl = await page.locator("text=Today's Story").first();
  if (await storyEl.count() > 0) {
    await storyEl.scrollIntoViewIfNeeded();
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${OUT}/03-todays-story.png`, fullPage: false });
  }

  // 6. Check Quick Capture
  console.log('\n=== CHECK 3: Quick Capture ===');
  const fixedEls = await page.evaluate(() => {
    const all = document.querySelectorAll('*');
    const fixed = [];
    for (const el of all) {
      const style = window.getComputedStyle(el);
      if (style.position === 'fixed') {
        fixed.push({
          tag: el.tagName,
          class: el.className?.toString()?.slice(0, 80),
          text: el.textContent?.trim()?.slice(0, 60),
          bottom: style.bottom,
          right: style.right,
          rect: el.getBoundingClientRect()
        });
      }
    }
    return fixed;
  });
  console.log('Fixed-position elements:', JSON.stringify(fixedEls, null, 2));

  // Look for capture button specifically
  const captureBtn = await page.locator('button').filter({ hasText: /capture|quick/i }).first();
  if (await captureBtn.count() > 0) {
    console.log('Found Quick Capture button');
    await captureBtn.screenshot({ path: `${OUT}/04-quick-capture-btn.png` });
    await captureBtn.click();
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${OUT}/04-quick-capture-open.png`, fullPage: false });
    // close it
    await page.keyboard.press('Escape');
    await page.waitForTimeout(300);
  } else {
    // Try alt+c
    console.log('No capture button found, trying Alt+C...');
    await page.keyboard.press('Alt+c');
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${OUT}/04-quick-capture-altc.png`, fullPage: false });
  }

  // 7. Check crew consolidation
  console.log('\n=== CHECK 4: Crew consolidation ===');
  const crewInfo = await page.evaluate(() => {
    const body = document.body.innerText;
    const lines = body.split('\n').filter(l => l.toLowerCase().includes('crew'));
    // Also look for crew-crew_N pattern
    const rawCrewPattern = body.match(/crew[-_]crew[-_]\d+/gi);
    return { crewLines: lines, rawCrewPatterns: rawCrewPattern };
  });
  console.log('Crew entries:', JSON.stringify(crewInfo, null, 2));

  // 8. Check "All quiet" empty state
  console.log('\n=== CHECK 5: Empty state ===');
  const emptyState = await page.evaluate(() => {
    const body = document.body.innerText;
    const hasAllQuiet = body.includes('All quiet') || body.includes('all quiet');
    // Also check Active Sessions section
    const sessionIdx = body.indexOf('Active Sessions');
    const sessionExcerpt = sessionIdx >= 0 ? body.slice(sessionIdx, sessionIdx + 300) : null;
    return { hasAllQuiet, sessionExcerpt };
  });
  console.log('Empty state:', JSON.stringify(emptyState, null, 2));

  // Find Active Sessions and screenshot
  const sessionsEl = await page.locator("text=Active Sessions").first();
  if (await sessionsEl.count() > 0) {
    await sessionsEl.scrollIntoViewIfNeeded();
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${OUT}/05-active-sessions.png`, fullPage: false });
  }

  // 9. Check grid / reachability of sections
  console.log('\n=== CHECK 6: Section reachability ===');
  const sectionReach = await page.evaluate(() => {
    const sections = ['Recent Sessions', 'Project Dashboard', 'Insights', 'Active Sessions'];
    const results = {};
    for (const name of sections) {
      const el = Array.from(document.querySelectorAll('h1, h2, h3, h4, h5, h6, [class*="heading"]')).find(h => h.textContent?.includes(name));
      if (el) {
        const rect = el.getBoundingClientRect();
        results[name] = { found: true, y: rect.y, visible: rect.y < document.documentElement.scrollHeight };
      } else {
        results[name] = { found: false };
      }
    }
    return results;
  });
  console.log('Section reachability:', JSON.stringify(sectionReach, null, 2));

  // Scroll down to reach all sections
  await page.evaluate(() => window.scrollTo(0, 0));
  for (let i = 1; i <= 8; i++) {
    await page.evaluate(() => window.scrollBy(0, window.innerHeight * 0.8));
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${OUT}/06-scroll-${i}.png`, fullPage: false });
  }

  // 10. Check grid responsive classes
  console.log('\n=== CHECK 6b: Grid classes ===');
  const gridClasses = await page.evaluate(() => {
    const grids = document.querySelectorAll('[class*="grid"], [class*="Grid"]');
    return Array.from(grids).map(g => ({
      class: g.className?.slice(0, 150),
      children: g.children.length,
      hasLgBreakpoint: g.className?.includes('lg:')
    }));
  });
  console.log('Grid layouts:', JSON.stringify(gridClasses, null, 2));

  // 11. Mobile viewport
  console.log('\n=== CHECK 7: Mobile responsiveness ===');
  await page.setViewportSize({ width: 375, height: 812 });
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.waitForTimeout(1000);
  await page.screenshot({ path: `${OUT}/07-mobile-landing.png`, fullPage: false });
  await page.screenshot({ path: `${OUT}/07-mobile-full.png`, fullPage: true });

  // 12. Try to click a PathTimeline group to test expand
  console.log('\n=== CHECK 8: PathTimeline expand/collapse ===');
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.waitForTimeout(500);

  // Look for clickable timeline project groups
  const clickableGroups = await page.evaluate(() => {
    // Try various selectors for timeline groups
    const selectors = [
      '[class*="timeline"] [class*="group"]',
      '[class*="timeline"] [class*="project"]',
      '[class*="timeline"] summary',
      '[class*="timeline"] details',
      '[class*="path"] [class*="group"]',
      'details',
      'summary'
    ];
    const results = [];
    for (const sel of selectors) {
      const els = document.querySelectorAll(sel);
      for (const el of els) {
        results.push({
          selector: sel,
          tag: el.tagName,
          class: el.className?.slice(0, 80),
          text: el.textContent?.trim()?.slice(0, 60)
        });
      }
    }
    return results;
  });
  console.log('Clickable timeline groups:', JSON.stringify(clickableGroups?.slice(0, 10), null, 2));

  // Also look for collapse/expand via Svelte component patterns
  const svelteComponents = await page.evaluate(() => {
    const els = document.querySelectorAll('[data-svelte-h], [class*="svelte"]');
    return Array.from(els).slice(0, 20).map(e => ({
      tag: e.tagName,
      class: e.className?.slice(0, 80),
      text: e.textContent?.trim()?.slice(0, 40)
    }));
  });
  console.log('Svelte components (sample):', JSON.stringify(svelteComponents?.slice(0, 5), null, 2));

  // 13. Check Insights sidebar
  console.log('\n=== CHECK 9: Sidebar Insights ===');
  const insightsInfo = await page.evaluate(() => {
    const body = document.body.innerText;
    const insightIdx = body.indexOf('Insights');
    if (insightIdx < 0) return { found: false };
    return { found: true, excerpt: body.slice(insightIdx, insightIdx + 300) };
  });
  console.log('Insights:', JSON.stringify(insightsInfo, null, 2));

  await browser.close();
  console.log('\n=== Done! Screenshots saved to', OUT, '===');
}

run().catch(e => { console.error(e); process.exit(1); });
