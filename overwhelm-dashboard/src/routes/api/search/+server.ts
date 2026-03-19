import { json } from '@sveltejs/kit';
import { searchPkbTasks } from '$lib/server/pkb';
import type { RequestHandler } from './$types';

/** GET /api/search?q=<query>&limit=10 — semantic task search via MCP */
export const GET: RequestHandler = async ({ url }) => {
    const q = url.searchParams.get('q')?.trim();
    const limit = parseInt(url.searchParams.get('limit') ?? '10', 10);
    const project = url.searchParams.get('project') ?? undefined;

    if (!q) {
        return json({ results: [] });
    }

    const text = await searchPkbTasks(q, { limit, project });
    if (text === null) {
        return json({ error: 'PKB unavailable' }, { status: 503 });
    }

    // Parse markdown results into structured objects
    // Format: "### N. Title (score: X.XXX)\n**Path:** `/path/to/file.md`\n**Status:** ...\n**Priority:** ..."
    const results: Array<{ id: string; title: string; status?: string; priority?: number; score: number }> = [];

    const blocks = text.split(/\n(?=### \d+\.)/).slice(1); // skip header line
    for (const block of blocks) {
        const titleMatch = block.match(/^### \d+\.\s+(.+?)\s+\(score:\s*([\d.]+)\)/);
        if (!titleMatch) continue;

        const title = titleMatch[1].trim();
        const score = parseFloat(titleMatch[2]);

        // Extract task ID from path: filename is "<id>-<slug>.md"
        const pathMatch = block.match(/\*\*Path:\*\*\s+`[^`]*\/([^`/]+)\.md`/);
        let id = '';
        if (pathMatch) {
            // ID is the portion before the first long hex suffix (8 hex chars after a dash)
            const filename = pathMatch[1];
            const idMatch = filename.match(/^([a-z][a-z0-9]+-[a-f0-9]{8})/);
            id = idMatch?.[1] ?? filename.split('-').slice(0, 2).join('-');
        }

        const statusMatch = block.match(/\*\*Status:\*\*\s+(\S+)/);
        const priorityMatch = block.match(/\*\*Priority:\*\*\s+(\d+)/);

        results.push({
            id,
            title,
            score,
            status: statusMatch?.[1],
            priority: priorityMatch ? parseInt(priorityMatch[1], 10) : undefined,
        });
    }

    return json({ results });
};
