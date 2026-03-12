import { test, expect } from '@playwright/test';
import * as fs from 'node:fs';
import * as path from 'node:path';

test('treemap quality metric: text visibility', async ({ page }) => {
  await page.goto('http://localhost:5173/');

  // Wait for the app to initialize
  await page.waitForSelector('text=SYSTEM READY', { timeout: 10000 });

  // Open the TASK GRAPH view
  await page.getByRole('button', { name: 'TASK GRAPH' }).click();

  // Ensure Treemap is active
  const treemapBtn = page.getByRole('button', { name: 'TREEMAP' });
  if (await treemapBtn.isVisible()) {
    await treemapBtn.click();
  }

  // Wait for nodes to render
  await page.waitForSelector('g.node rect', { state: 'visible', timeout: 10000 });
  await page.waitForTimeout(1000); // give layout a moment to settle

  // Extract dimensions of all nodes
  const metrics = await page.evaluate(() => {
    const nodes = Array.from(document.querySelectorAll('g.node'));
    let totalNodes = 0;
    let textVisibleCount = 0;
    let textHiddenCount = 0;
    let tallNodesCount = 0;
    
    // Thresholds matching NodeShapes.ts
    const MIN_W = 15;
    const MIN_H = 10;
    const ASPECT_RATIO_W_OVER_H = 0.7;

    nodes.forEach(n => {
      const rect = n.querySelector('rect');
      // Skip synthetic/overflow/root nodes if they don't have rects with width/height
      if (!rect || !rect.hasAttribute('width')) return;
      
      const w = parseFloat(rect.getAttribute('width') || '0');
      const h = parseFloat(rect.getAttribute('height') || '0');
      
      if (w <= 0 || h <= 0) return; // Skip invisible nodes

      totalNodes++;
      
      if (h > w) {
        tallNodesCount++;
      }

      // Replicate the logic from NodeShapes.ts
      if (w > MIN_W && h > MIN_H && (w >= h * ASPECT_RATIO_W_OVER_H)) {
        textVisibleCount++;
      } else {
        textHiddenCount++;
      }
    });

    return {
      timestamp: new Date().toISOString(),
      totalNodes,
      textVisibleCount,
      textHiddenCount,
      tallNodesCount,
      textVisibilityPercentage: totalNodes > 0 ? (textVisibleCount / totalNodes) * 100 : 0,
    };
  });

  console.log(`Treemap Metrics:`, metrics);

  // Store the metric
  const qaDir = path.join(process.cwd(), 'qa');
  if (!fs.existsSync(qaDir)) {
    fs.mkdirSync(qaDir, { recursive: true });
  }
  
  const metricsFile = path.join(qaDir, 'treemap-metrics.jsonl');
  fs.appendFileSync(metricsFile, JSON.stringify(metrics) + '\n');

  // We can assert a baseline quality threshold if we want the test to fail when it gets too bad
  // expect(metrics.textVisibilityPercentage).toBeGreaterThan(20);
});
