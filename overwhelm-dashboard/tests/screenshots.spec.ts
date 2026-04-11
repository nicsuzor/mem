import { test, expect } from '@playwright/test';
import * as path from 'node:path';
import * as fs from 'node:fs';

test('capture screenshots of major components', async ({ page }) => {
  test.setTimeout(90_000);
  // Set viewport to a good standard size for desktop dashboards
  await page.setViewportSize({ width: 1920, height: 1080 });

  await page.goto('http://localhost:5173/');

  // Create screenshots directory
  const aopsSessions = process.env.AOPS_SESSIONS;
  if (!aopsSessions) {
    console.warn('AOPS_SESSIONS environment variable not set. Falling back to local qa/overwhelm directory.');
  }
  const screenshotsDir = aopsSessions 
    ? path.join(aopsSessions, 'qa', 'overwhelm') 
    : path.join(process.cwd(), 'qa', 'overwhelm');

  if (!fs.existsSync(screenshotsDir)) {
    fs.mkdirSync(screenshotsDir, { recursive: true });
  }

  // Wait for the initial system loading to finish
  await page.waitForSelector('text=OPERATOR SYSTEM', { timeout: 15000 });
  await page.waitForTimeout(2000); // Give dashboard extra time to render data

  // 1. Dashboard Tab (Default view)
  console.log('Capturing Dashboard view...');
  await page.getByRole('button', { name: 'DASHBOARD', exact: true }).click();
  await page.waitForTimeout(1000); 
  await page.screenshot({ path: path.join(screenshotsDir, '01-dashboard.png'), fullPage: true });

  // 2. Task Graph - Treemap
  console.log('Capturing Treemap view...');
  await page.getByRole('button', { name: 'Treemap' }).click();
  await page.waitForSelector('g.node rect', { state: 'visible', timeout: 15000 });
  await page.waitForTimeout(1500); // Let layout settle
  await page.screenshot({ path: path.join(screenshotsDir, '02-treemap.png') });

  // 3. Task Graph - Circle Pack
  console.log('Capturing Circle Pack view...');
  await page.getByRole('button', { name: 'Circle Pack' }).click();
  await page.waitForSelector('g.node circle', { state: 'visible', timeout: 15000 });
  await page.waitForTimeout(1500);
  await page.screenshot({ path: path.join(screenshotsDir, '03-circle-pack.png') });

  // 4. Task Graph - Force Directed
  console.log('Capturing Force Directed view...');
  await page.getByRole('button', { name: 'Force' }).click();
  await page.waitForSelector('g.node', { state: 'visible', timeout: 15000 });
  await page.waitForTimeout(4000); // Force layout takes longer to settle
  await page.screenshot({ path: path.join(screenshotsDir, '04-force-directed.png') });

  // 5. Task Graph - Metro
  console.log('Capturing Metro view...');
  await page.getByRole('button', { name: 'Metro' }).click();
  await page.waitForSelector('[data-component="metro-map"] canvas', { state: 'visible', timeout: 15000 });
  await page.waitForTimeout(2000);
  await page.screenshot({ path: path.join(screenshotsDir, '05-metro.png') });

  // 6. Task Graph - Arc Diagram
  console.log('Capturing Arc Diagram view...');
  await page.getByRole('button', { name: 'Arc Diagram' }).click();
  await page.waitForSelector('g.node', { state: 'visible', timeout: 15000 });
  await page.waitForTimeout(2000);
  await page.screenshot({ path: path.join(screenshotsDir, '06-arc-diagram.png') });

  // 7. Threaded Tasks
  console.log('Capturing Threaded Tasks view...');
  await page.getByRole('button', { name: 'THREADED TASKS' }).click();
  await page.waitForTimeout(1500);
  await page.screenshot({ path: path.join(screenshotsDir, '07-threaded-tasks.png'), fullPage: true });

  // 8. View Config Overlay (Open it in Treemap and screenshot)
  console.log('Capturing Config Overlay...');
  await page.getByRole('button', { name: 'Treemap' }).click();
  await page.waitForTimeout(1000);
  await page.locator('.config-toggle').click();
  await page.waitForTimeout(500); // Wait for panel to open
  await page.screenshot({ path: path.join(screenshotsDir, '08-config-overlay.png') });
});
