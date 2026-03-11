# QA Assessment: Treemap Visualization (Regression)

**Date:** 2026-03-11
**Evaluator:** Agent QA (Playwright-assisted)
**Target:** Overwhelm Dashboard - Treemap View
**Task ID:** overwhelm-dashboard-25c09d4f

## Verdict: ❌ FAILED (Semantic Integrity Violation)

### 1. Visual Inspection
I captured a runtime snapshot of the Treemap view (`overwhelm-dashboard/qa/screenshots/treemap-2026-03-11.png`) after the latest layout adjustments.

**Observations:**
- The right sidebar is correctly docked and rendering the "SYSTEM READY" telemetry pane.
- The top-level filters and view toggles are working.
- The Treemap rendering area is completely filled with **extremely thin, horizontal green bars** spanning the entire width of the main column. 

### 2. Qualitative Analysis

The user's instructions contained two conflicting constraints:
1. *"fully prohibit treemap nodes that are taller than they are wide"* (Technical Constraint)
2. *"yes, there are no more tall boxes. but also it's not a tree map anymore."* (Semantic Constraint)

**The Mathematical Conflict:**
It is mathematically impossible to pack a deep, arbitrary hierarchy of hundreds of nodes into a fixed 2D rectangle (with $W \ge H$ strictly enforced for *every* node) without devolving into a 1-dimensional "slice" layout (a stacked bar chart). 

By strictly enforcing the $W \ge H$ rule in the custom D3 tiling algorithm, the layout engine had no choice but to recursively stack horizontal rows. The result is a 1D vertical list of extremely thin bars. 

**Assessment against dimensions:**
- **Output Quality:** POOR. The nodes are so thin that text rendering is impossible. The data density is high, but the legibility is zero.
- **Process Compliance:** STRICT. The technical constraint ($W \ge H$) was followed to the letter.
- **Semantic Correctness:** FAILED. The user explicitly called out that a 1D list "is not a tree map anymore." The current implementation suffers from the exact same "Monkey's Paw" compliance failure that triggered the `/learn` incident earlier. It has lost its 2D spatial identity.

### 3. Root Cause
The custom `tile` function in `TreemapView.svelte` forces a horizontal split (`d3.treemapSlice`) whenever a vertical split (`d3.treemapDice`) would result in a tall node. Given the dataset size (732 nodes) and the container dimensions (2000x1200), forcing width-bias at the leaf level inevitably crushes the height of the nodes into sub-pixel slivers.

### 4. Recommendation for Engineering
The strict $W \ge H$ constraint must be relaxed or re-interpreted. To restore the 2D Treemap identity while favoring scannable wide nodes, the engineer must:

1. Revert to `d3.treemapSquarify` to restore 2D nesting.
2. Accept that some nodes *must* be taller than wide to satisfy the 2D packing algorithm.
3. Instead of *prohibiting* tall nodes at the layout level, address the *symptom* (which is likely ugly vertical text or awkward bounding boxes) by:
   - Hiding text on tall nodes.
   - Using a hybrid layout where only the top 1 or 2 levels are forced into rows (Slice), and the leaf nodes are squarified.
   - Rotating text for tall nodes.

**Next Step:** The implementation must be rolled back to a true 2D `treemapSquarify` layout, and the "no tall nodes" constraint must be treated as a *strong preference* rather than an absolute mathematical prohibition, or handled via CSS/Text rendering rules.