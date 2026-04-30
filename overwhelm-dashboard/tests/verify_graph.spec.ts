import { test, expect } from '@playwright/test';

test('Verify graph view and interactions', async ({ page }) => {
  await page.goto('http://localhost:5173');
  
  // 1. Verify top navigation menu has DASHBOARD, GRAPH, and TASKS
  await expect(page.getByRole('button', { name: 'DASHBOARD' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'GRAPH' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'TASKS' })).toBeVisible();
  
  // 2. Click GRAPH
  await page.getByRole('button', { name: 'GRAPH' }).click();
  
  // Verify that a "Views" sub-navigation bar appears with buttons like Treemap, Force, etc.
  // The word "Views" might be a label or heading.
  await expect(page.getByText('Views', { exact: false })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Treemap' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Force' })).toBeVisible();
  
  // 3. Click one of the view buttons (e.g. Force). Verify it gets highlighted.
  const forceButton = page.getByRole('button', { name: 'Force' });
  await forceButton.click();
  
  // Verification of highlight: usually this means a specific class or style.
  // Without knowing the exact implementation, I'll check if it has an 'active' class or something similar,
  // or just check if it's still visible and perhaps has some attribute.
  // Many Svelte/Tailwind apps use class names like 'bg-blue-500' or similar for active states.
  // I'll check for a common pattern or just assert it's clicked.
  // Actually, I'll inspect the element's classes.
  await expect(forceButton).toHaveClass(/bg-primary\/15/);
  
  // 4. Take a screenshot of the new layout.
  await page.screenshot({ path: '/workspace/graph_layout.png', fullPage: true });
  
  // 5. Verify the status filter bar is still visible at the top of the graph area.
  // I'll look for text like "Filters" or status names like "Ready", "Blocked", "Done" if they are common.
  // Or look for a container that looks like a filter bar.
  await expect(page.getByRole('toolbar', { name: 'Status filter' })).toBeVisible();
});
