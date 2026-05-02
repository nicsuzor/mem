import { test, expect } from '@playwright/test';

test('capture screenshot and check nav', async ({ page }) => {
  await page.goto('http://localhost:5173');
  
  // Wait for the page to load
  await page.waitForLoadState('networkidle');
  
  // Take screenshot
  await page.screenshot({ path: '/workspace/nav_menu.png', fullPage: true });
  
  // Find nav buttons
  const navButtons = await page.evaluate(() => {
    const nav = document.querySelector('nav') || document.querySelector('header');
    if (!nav) return [];
    return Array.from(nav.querySelectorAll('button, a'))
      .map(el => (el as HTMLElement).innerText.trim())
      .filter(t => t.length > 0);
  });
  
  const expectedButtons = ['DASHBOARD', 'GRAPH', 'TASKS', 'Menu'];
  expect(navButtons).toEqual(expect.arrayContaining(expectedButtons));
});
