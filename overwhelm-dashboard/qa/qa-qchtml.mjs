import { chromium } from 'playwright';

const URL = 'http://100.115.15.120:5173';

async function run() {
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  await page.goto(URL, { waitUntil: 'networkidle', timeout: 30000 });
  await page.waitForTimeout(3000);

  // Get the raw HTML of the QuickCapture area
  const result = await page.evaluate(() => {
    // Find the edit_note span
    const spans = document.querySelectorAll('span');
    for (const span of spans) {
      if (span.textContent?.trim() === 'edit_note' || span.textContent?.trim() === 'close') {
        // Walk up to find the button parent
        let btn = span.closest('button');
        if (btn) {
          return {
            found: true,
            btnHTML: btn.outerHTML?.slice(0, 500),
            btnClass: btn.className,
            btnStyle: btn.getAttribute('style'),
            parentHTML: btn.parentElement?.outerHTML?.slice(0, 800)
          };
        }
      }
    }
    
    // Alternative: find CAPTURE NOTE button and look for siblings
    const captureBtn = Array.from(document.querySelectorAll('button')).find(b => b.textContent?.includes('CAPTURE NOTE'));
    if (captureBtn) {
      const container = captureBtn.closest('div[class*="flex"]');
      const parent = container?.parentElement;
      // Look for sibling buttons
      const siblings = parent?.parentElement?.querySelectorAll('button');
      return {
        found: false,
        captureNoteBtn: captureBtn.outerHTML?.slice(0, 300),
        containerClass: container?.className,
        parentClass: parent?.className,
        parentHTML: parent?.outerHTML?.slice(0, 1000),
        siblingCount: siblings?.length,
        parentParentClass: parent?.parentElement?.className?.slice(0, 100)
      };
    }
    
    return { found: false };
  });

  console.log('QuickCapture HTML:', JSON.stringify(result, null, 2));

  await browser.close();
}

run().catch(e => { console.error(e); process.exit(1); });
