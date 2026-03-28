import { chromium } from 'playwright';

const URL = 'http://100.115.15.120:5173';

async function run() {
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  await page.goto(URL, { waitUntil: 'networkidle', timeout: 30000 });
  await page.waitForTimeout(3000);

  // Find ALL buttons and check which ones have "fixed" in tailwind class
  const result = await page.evaluate(() => {
    const allBtns = document.querySelectorAll('button');
    for (const btn of allBtns) {
      const cls = btn.className || '';
      if (cls.includes('fixed')) {
        const computed = getComputedStyle(btn);
        // Walk up ancestors looking for transform/filter/will-change
        const ancestors = [];
        let el = btn.parentElement;
        while (el && el !== document.body) {
          const s = getComputedStyle(el);
          if (s.transform !== 'none' || s.willChange !== 'auto' || s.filter !== 'none' || s.contain !== 'none') {
            ancestors.push({
              tag: el.tagName,
              class: el.className?.slice(0, 100),
              transform: s.transform,
              willChange: s.willChange,
              filter: s.filter,
              contain: s.contain
            });
          }
          el = el.parentElement;
        }
        return {
          found: true,
          btnClass: cls.slice(0, 120),
          computedPosition: computed.position,
          computedBottom: computed.bottom,
          computedRight: computed.right,
          ancestorsWithContainingBlock: ancestors,
          isVisible: btn.offsetParent !== null,
          rect: btn.getBoundingClientRect()
        };
      }
    }
    
    // Also check if QuickCapture is rendered at component level
    // Look for the edit_note icon text
    const allEls = document.querySelectorAll('span');
    let editNoteFound = false;
    for (const el of allEls) {
      if (el.textContent?.trim() === 'edit_note') {
        editNoteFound = true;
        break;
      }
    }
    
    return { found: false, editNoteSpanFound: editNoteFound };
  });

  console.log('Fixed button check:', JSON.stringify(result, null, 2));

  // Check if the QuickCapture component is even in the DOM
  const qcInDom = await page.evaluate(() => {
    const body = document.body.innerHTML;
    return {
      hasFixedClass: body.includes('fixed bottom-6 right-6'),
      hasQuickCaptureTitle: body.includes('Quick Capture'),
      hasEditNote: body.includes('edit_note'),
      allFixedElements: Array.from(document.querySelectorAll('[class*="fixed"]')).map(el => ({
        tag: el.tagName,
        class: el.className?.slice(0, 100),
        position: getComputedStyle(el).position
      }))
    };
  });
  console.log('QC DOM check:', JSON.stringify(qcInDom, null, 2));

  await browser.close();
}

run().catch(e => { console.error(e); process.exit(1); });
