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

  // Landing
  await page.screenshot({ path: `${OUT}/01-landing.png`, fullPage: false });
  await page.screenshot({ path: `${OUT}/02-full-page.png`, fullPage: true });

  // Section headers
  const headers = await page.evaluate(() => {
    const hs = document.querySelectorAll('h1, h2, h3, h4, h5, h6');
    return Array.from(hs).map(h => `${h.tagName}: ${h.textContent?.trim()?.slice(0, 100)}`);
  });
  console.log('HEADERS:', JSON.stringify(headers, null, 2));

  // CHECK 1: PathTimeline / PATH RECONSTRUCTION
  console.log('\n=== CHECK 1: Path Reconstruction ===');
  const pathEl = await page.locator("text=PATH RECONSTRUCTION").first();
  if (await pathEl.count() > 0) {
    await pathEl.scrollIntoViewIfNeeded();
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${OUT}/10-path-reconstruction.png`, fullPage: false });
    
    // Count project groups under this section
    const pathGroupInfo = await page.evaluate(() => {
      const heading = Array.from(document.querySelectorAll('h3')).find(h => h.textContent?.includes('PATH RECONSTRUCTION'));
      if (!heading) return { found: false };
      const container = heading.closest('section') || heading.parentElement;
      // count children that look like project groups
      const text = container?.textContent?.slice(0, 2000);
      // Look for show more / collapse patterns
      const showMore = container?.querySelector('[class*="show"], button');
      return { 
        found: true, 
        textPreview: text?.slice(0, 500),
        hasShowMore: !!showMore,
        showMoreText: showMore?.textContent?.trim()
      };
    });
    console.log('Path Reconstruction info:', JSON.stringify(pathGroupInfo, null, 2));
  } else {
    console.log('PATH RECONSTRUCTION heading not found');
  }

  // CHECK 2: Today's Story
  console.log("\n=== CHECK 2: Today's Story ===");
  const storyEl = await page.locator("text=TODAY'S STORY").first();
  if (await storyEl.count() > 0) {
    await storyEl.scrollIntoViewIfNeeded();
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${OUT}/11-todays-story.png`, fullPage: false });
    
    const storyContent = await page.evaluate(() => {
      const heading = Array.from(document.querySelectorAll('h3')).find(h => h.textContent?.includes("TODAY'S STORY"));
      if (!heading) return { found: false };
      const container = heading.closest('section') || heading.parentElement;
      const text = container?.textContent?.trim()?.slice(0, 1500);
      // Check for project grouping (brackets indicate project names)
      const projectBrackets = text?.match(/\[[\w\s]+\]/g);
      // Check for badges
      const badges = container?.querySelectorAll('[class*="badge"], [class*="tag"], span[class*="inline"]');
      return { 
        found: true, 
        textPreview: text?.slice(0, 800),
        projectGroups: projectBrackets,
        badgeCount: badges?.length || 0
      };
    });
    console.log("Today's Story:", JSON.stringify(storyContent, null, 2));
  }

  // CHECK 3: Quick Capture - look for floating overlay
  console.log('\n=== CHECK 3: Quick Capture ===');
  // Check if there's a floating capture button (fixed position at bottom-right)
  const captureInfo = await page.evaluate(() => {
    // Look for the Quick Capture section
    const heading = Array.from(document.querySelectorAll('h3')).find(h => h.textContent?.includes('QUICK CAPTURE'));
    const allFixedEls = [];
    document.querySelectorAll('*').forEach(el => {
      const style = getComputedStyle(el);
      if (style.position === 'fixed' && el.textContent?.trim()) {
        allFixedEls.push({
          tag: el.tagName,
          class: el.className?.toString()?.slice(0, 80),
          text: el.textContent?.trim()?.slice(0, 60),
          rect: el.getBoundingClientRect()
        });
      }
    });
    
    // Is quick capture in sidebar or floating?
    const qcLocation = heading ? {
      found: true,
      parentClass: heading.parentElement?.className?.slice(0, 100),
      grandparentClass: heading.parentElement?.parentElement?.className?.slice(0, 100),
      isInSidebar: heading.closest('[class*="sidebar"], [class*="side"], aside') !== null,
      isFixed: getComputedStyle(heading.closest('div') || heading).position === 'fixed'
    } : { found: false };
    
    return { fixedElements: allFixedEls, quickCapture: qcLocation };
  });
  console.log('Quick Capture info:', JSON.stringify(captureInfo, null, 2));

  // Screenshot Quick Capture section
  const qcHeading = await page.locator("text=QUICK CAPTURE").first();
  if (await qcHeading.count() > 0) {
    await qcHeading.scrollIntoViewIfNeeded();
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${OUT}/12-quick-capture.png`, fullPage: false });
  }

  // CHECK 4: Crew consolidation
  console.log('\n=== CHECK 4: Crew consolidation ===');
  const crewInfo = await page.evaluate(() => {
    const body = document.body.innerText;
    const crewPatterns = body.match(/crew[-_]crew[-_]\d+/gi);
    const crewLines = body.split('\n').filter(l => l.toLowerCase().includes('crew')).slice(0, 10);
    return { unconsolidatedPatterns: crewPatterns, crewLines };
  });
  console.log('Crew info:', JSON.stringify(crewInfo, null, 2));

  // CHECK 5: Active Sessions / "All quiet"
  console.log('\n=== CHECK 5: Active Sessions empty state ===');
  const activeSessionEl = await page.locator("text=CURRENT ACTIVITY").first();
  if (await activeSessionEl.count() > 0) {
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${OUT}/13-current-activity.png`, fullPage: false });
  }
  
  const sessionInfo = await page.evaluate(() => {
    const body = document.body.innerText;
    const hasAllQuiet = body.includes('All quiet') || body.includes('all quiet');
    const hasMoonIcon = body.includes('🌙') || document.querySelector('[class*="moon"]') !== null;
    
    // Check Current Activity content
    const actHeader = Array.from(document.querySelectorAll('h3')).find(h => h.textContent?.includes('CURRENT ACTIVITY'));
    const actContent = actHeader?.closest('section')?.textContent?.trim()?.slice(0, 500) || actHeader?.parentElement?.textContent?.trim()?.slice(0, 500);
    
    return { hasAllQuiet, hasMoonIcon, activityContent: actContent };
  });
  console.log('Session info:', JSON.stringify(sessionInfo, null, 2));

  // CHECK 6: Recent Sessions and Project Dashboard reachability
  console.log('\n=== CHECK 6: Section reachability ===');
  const recentSessEl = await page.locator("text=RECENT SESSIONS").first();
  if (await recentSessEl.count() > 0) {
    await recentSessEl.scrollIntoViewIfNeeded();
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${OUT}/14-recent-sessions.png`, fullPage: false });
    console.log('Recent Sessions: REACHABLE');
  } else {
    console.log('Recent Sessions: NOT FOUND');
  }

  // Check for project dashboard sections (folder_open headers)
  const projectDashHeaders = await page.evaluate(() => {
    const hs = Array.from(document.querySelectorAll('h3')).filter(h => h.textContent?.includes('folder_open'));
    return hs.map(h => h.textContent?.trim()?.slice(0, 80));
  });
  console.log('Project Dashboard sections:', JSON.stringify(projectDashHeaders));

  if (projectDashHeaders.length > 0) {
    // Scroll to the first one and screenshot
    const firstProject = await page.locator("h3:has-text('folder_open')").first();
    await firstProject.scrollIntoViewIfNeeded();
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${OUT}/15-project-dashboard.png`, fullPage: false });
    console.log('Project Dashboard: REACHABLE');
  }

  // CHECK 6b: Grid/responsive layout
  console.log('\n=== CHECK 6b: Grid layout ===');
  const gridInfo = await page.evaluate(() => {
    const grids = document.querySelectorAll('[class*="grid"]');
    return Array.from(grids).map(g => ({
      class: g.className?.slice(0, 150),
      children: g.children.length,
      hasLg: g.className?.includes('lg:')
    }));
  });
  console.log('Grid classes:', JSON.stringify(gridInfo, null, 2));

  // CHECK 7: Insights sidebar
  console.log('\n=== CHECK 7: Insights ===');
  const insightsEl = await page.locator("text=INSIGHTS").first();
  if (await insightsEl.count() > 0) {
    await insightsEl.scrollIntoViewIfNeeded();
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${OUT}/16-insights.png`, fullPage: false });
    
    const insightsContent = await page.evaluate(() => {
      const heading = Array.from(document.querySelectorAll('h3')).find(h => h.textContent?.includes('INSIGHTS'));
      const container = heading?.closest('section') || heading?.parentElement;
      return container?.textContent?.trim()?.slice(0, 300);
    });
    console.log('Insights content:', insightsContent);
  } else {
    console.log('Insights: NOT FOUND (may be hidden if no unique content)');
  }

  // Scroll captures
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.waitForTimeout(300);
  for (let i = 1; i <= 10; i++) {
    await page.evaluate(() => window.scrollBy(0, window.innerHeight * 0.8));
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${OUT}/20-scroll-${String(i).padStart(2,'0')}.png`, fullPage: false });
  }

  // CHECK 8: Mobile
  console.log('\n=== CHECK 8: Mobile responsiveness ===');
  await page.setViewportSize({ width: 375, height: 812 });
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.waitForTimeout(1500);
  await page.screenshot({ path: `${OUT}/30-mobile-landing.png`, fullPage: false });
  await page.screenshot({ path: `${OUT}/30-mobile-full.png`, fullPage: true });

  // Mobile scroll captures
  for (let i = 1; i <= 5; i++) {
    await page.evaluate(() => window.scrollBy(0, window.innerHeight * 0.8));
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${OUT}/31-mobile-scroll-${i}.png`, fullPage: false });
  }

  await browser.close();
  console.log('\n=== QA Complete ===');
}

run().catch(e => { console.error(e); process.exit(1); });
