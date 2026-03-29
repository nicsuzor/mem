import { chromium } from 'playwright';

const URL = 'http://100.115.15.120:5173';

async function run() {
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  await page.goto(URL, { waitUntil: 'networkidle', timeout: 30000 });
  await page.waitForTimeout(3000);

  // Check if crew-crew_N appears unconsolidated in project dashboard headers
  const projectHeaders = await page.evaluate(() => {
    const hs = Array.from(document.querySelectorAll('h3')).filter(h => h.textContent?.includes('folder_open'));
    return hs.map(h => h.textContent?.trim()?.slice(0, 80));
  });
  console.log('Project Dashboard headers:', JSON.stringify(projectHeaders, null, 2));
  
  // Check if "lg:" responsive classes exist in the grid
  const gridCols = await page.evaluate(() => {
    const el = document.querySelector('[class*="grid-cols-12"]');
    return el ? {
      class: el.className,
      children: Array.from(el.children).map(c => ({ tag: c.tagName, class: c.className?.slice(0, 100) }))
    } : null;
  });
  console.log('Main grid:', JSON.stringify(gridCols, null, 2));

  // Check if the sidebar (col-span-4) includes QuickCapture OR Insights
  const sidebarContent = await page.evaluate(() => {
    const sidebar = document.querySelector('[class*="col-span-4"]');
    if (!sidebar) return { found: false };
    const children = Array.from(sidebar.children).map(c => ({
      tag: c.tagName,
      class: c.className?.slice(0, 100),
      headings: Array.from(c.querySelectorAll('h3')).map(h => h.textContent?.trim()?.slice(0, 50))
    }));
    return { found: true, children };
  });
  console.log('Sidebar:', JSON.stringify(sidebarContent, null, 2));

  await browser.close();
}

run().catch(e => { console.error(e); process.exit(1); });
