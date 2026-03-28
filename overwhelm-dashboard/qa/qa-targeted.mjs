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

  // 1. Quick Capture: check if the fixed button is rendered inside the overflow container
  console.log('=== Quick Capture position debug ===');
  const qcDebug = await page.evaluate(() => {
    // The QuickCapture component renders a button with class "fixed bottom-6 right-6 z-50"
    const btns = document.querySelectorAll('button');
    const fixedBtns = [];
    for (const btn of btns) {
      const cls = btn.className || '';
      if (cls.includes('fixed') || cls.includes('z-50')) {
        const style = getComputedStyle(btn);
        const parent = btn.parentElement;
        const grandparent = parent?.parentElement;
        fixedBtns.push({
          class: cls.slice(0, 120),
          computedPosition: style.position,
          computedBottom: style.bottom,
          computedRight: style.right,
          computedZIndex: style.zIndex,
          parentTag: parent?.tagName,
          parentClass: parent?.className?.slice(0, 80),
          parentOverflow: parent ? getComputedStyle(parent).overflow : 'n/a',
          grandparentOverflow: grandparent ? getComputedStyle(grandparent).overflow : 'n/a',
          // Check if any ancestor has overflow hidden/auto/scroll which clips fixed children
          isClipped: (() => {
            let el = btn.parentElement;
            while (el) {
              const ov = getComputedStyle(el).overflow;
              if (ov === 'hidden' || ov === 'auto' || ov === 'scroll') {
                const transform = getComputedStyle(el).transform;
                const willChange = getComputedStyle(el).willChange;
                if (transform !== 'none' || willChange === 'transform') {
                  return `clipped by ${el.tagName}.${el.className?.slice(0,60)} (transform/willChange)`;
                }
              }
              el = el.parentElement;
            }
            return false;
          })(),
          rect: btn.getBoundingClientRect()
        });
      }
    }
    return fixedBtns;
  });
  console.log('Fixed buttons:', JSON.stringify(qcDebug, null, 2));

  // 2. Check if QuickCapture is rendered at all
  const qcExists = await page.evaluate(() => {
    const allBtns = document.querySelectorAll('button');
    return Array.from(allBtns).map(b => ({
      text: b.textContent?.trim()?.slice(0, 30),
      class: b.className?.slice(0, 80)
    })).filter(b => b.class?.includes('fixed') || b.text?.toLowerCase().includes('capture') || b.class?.includes('z-50'));
  });
  console.log('Capture-related buttons:', JSON.stringify(qcExists, null, 2));

  // 3. Check the rendering hierarchy
  const hierarchy = await page.evaluate(() => {
    // Find any element with text "edit_note" (material icon for Quick Capture button)
    const el = document.querySelector('button[title*="Quick Capture"]');
    if (!el) return { found: false, allTitles: Array.from(document.querySelectorAll('button[title]')).map(b => b.title) };
    
    const ancestors = [];
    let current = el;
    while (current && current !== document.body) {
      ancestors.push({
        tag: current.tagName,
        class: current.className?.toString()?.slice(0, 80),
        overflow: getComputedStyle(current).overflow,
        position: getComputedStyle(current).position
      });
      current = current.parentElement;
    }
    return { found: true, ancestors };
  });
  console.log('QuickCapture hierarchy:', JSON.stringify(hierarchy, null, 2));

  // 4. Check lg: breakpoints in use
  console.log('\n=== Grid lg: breakpoint check ===');
  const lgClasses = await page.evaluate(() => {
    const allEls = document.querySelectorAll('*');
    const lgEls = [];
    for (const el of allEls) {
      if (el.className?.toString()?.includes('lg:')) {
        lgEls.push({
          tag: el.tagName,
          class: el.className?.slice(0, 150)
        });
      }
    }
    return lgEls;
  });
  console.log('Elements with lg: breakpoints:', JSON.stringify(lgClasses, null, 2));

  // 5. Check crew names that appear in RECENT SESSIONS area
  console.log('\n=== Crew in Recent Sessions ===');
  const recentSessionsCrew = await page.evaluate(() => {
    const heading = Array.from(document.querySelectorAll('h3')).find(h => h.textContent?.includes('RECENT SESSIONS'));
    if (!heading) return { found: false };
    const container = heading.closest('div[class*="border"]') || heading.parentElement;
    const text = container?.textContent || '';
    const crewMatches = text.match(/crew[-_]\w+/gi);
    return { found: true, crewMatches };
  });
  console.log('Recent Sessions crew:', JSON.stringify(recentSessionsCrew, null, 2));

  // 6. Today's Story badge check
  console.log("\n=== Today's Story badges ===");
  const storyBadges = await page.evaluate(() => {
    const heading = Array.from(document.querySelectorAll('h3')).find(h => h.textContent?.includes("TODAY'S STORY"));
    if (!heading) return { found: false };
    const section = heading.closest('div[class*="border"]');
    // Check for inline badges
    const badges = section?.querySelectorAll('[class*="badge"], span[class*="bg-"]');
    const alignBadge = section?.querySelector(':scope *')?.closest('div')?.querySelector('[class*="ALIGNMENT"], [class*="alignment"]');
    
    // Check for specific badge keywords
    const text = section?.textContent || '';
    return {
      found: true,
      hasAlignment: text.includes('ALIGNMENT'),
      hasBlockers: text.includes('BLOCKERS'),
      hasContext: text.includes('CONTEXT'),
      hasStale: text.includes('STALE'),
      badgeCount: badges?.length || 0,
      textPreview: text.slice(0, 300)
    };
  });
  console.log("Today's Story badges:", JSON.stringify(storyBadges, null, 2));

  // 7. Take a screenshot at the bottom to see RECENT SESSIONS + QUICK CAPTURE
  console.log('\n=== Bottom section screenshot ===');
  // Scroll to bottom of main section
  await page.evaluate(() => {
    const section = document.querySelector('section[class*="overflow"]');
    if (section) section.scrollTop = section.scrollHeight - section.clientHeight;
  });
  await page.waitForTimeout(500);
  await page.screenshot({ path: `${OUT}/60-bottom.png`, fullPage: false });

  await browser.close();
  console.log('\nDone');
}

run().catch(e => { console.error(e); process.exit(1); });
