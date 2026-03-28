import { chromium } from 'playwright';

const URL = 'http://100.115.15.120:5173';

async function run() {
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  await page.goto(URL, { waitUntil: 'networkidle', timeout: 30000 });
  await page.waitForTimeout(3000);

  // Check JS source for the floating button code
  const result = await page.evaluate(() => {
    // Check the entire page HTML for "fixed bottom" pattern
    const html = document.documentElement.innerHTML;
    const hasFixedBottom = html.includes('fixed bottom');
    const hasZ50 = html.includes('z-50');
    
    // Check loaded scripts for QuickCapture changes
    const scripts = document.querySelectorAll('script[src]');
    const scriptSrcs = Array.from(scripts).map(s => s.src);
    
    // Check if Alt+C keydown handler exists
    // We can test by seeing if the window has event listeners
    
    return { hasFixedBottom, hasZ50, scriptSrcs };
  });
  console.log('Deployed code check:', JSON.stringify(result, null, 2));
  
  // Check DashboardView sidebar content - is QuickCapture rendered there?
  const sidebarQC = await page.evaluate(() => {
    // The sidebar is col-span-4
    const sidebar = document.querySelector('[class*="col-span-4"]');
    if (!sidebar) return { sidebarFound: false };
    return {
      sidebarFound: true,
      hasQuickCapture: sidebar.textContent?.includes('QUICK CAPTURE'),
      sidebarHTML: sidebar.innerHTML?.slice(0, 1000)
    };
  });
  console.log('Sidebar QC:', JSON.stringify(sidebarQC, null, 2));
  
  // Also check if the DashboardView renders QC at the top level
  const topLevelQC = await page.evaluate(() => {
    // The DashboardView has class "h-full p-8 font-mono"
    const dashDiv = document.querySelector('.h-full.p-8.font-mono');
    if (!dashDiv) return { dashFound: false };
    // The first child should be the floating QuickCapture
    // But actually QuickCapture is rendered BEFORE the div
    const section = dashDiv.closest('section');
    const sectionChildren = section ? Array.from(section.children).map(c => ({
      tag: c.tagName,
      class: c.className?.slice(0, 80),
      text: c.textContent?.trim()?.slice(0, 50)
    })) : [];
    return { dashFound: true, sectionChildren };
  });
  console.log('Top-level QC:', JSON.stringify(topLevelQC, null, 2));

  await browser.close();
}

run().catch(e => { console.error(e); process.exit(1); });
