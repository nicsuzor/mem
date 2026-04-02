import { json } from '@sveltejs/kit';
import { fetchPkbGraph } from '$lib/server/pkb';
import type { RequestHandler } from './$types';

/** GET /api/graph — serve the single graph JSON via MCP */
export const GET: RequestHandler = async () => {
    try {
        const text = await fetchPkbGraph();
        if (text === null) {
            return json({ error: 'PKB unavailable or graph_json tool failed' }, { status: 503 });
        }
        
        return new Response(text, {
            headers: { 'Content-Type': 'application/json' },
        });
    } catch (err) {
        console.error('[api/graph] Unexpected error:', err);
        return json({ error: 'Internal server error' }, { status: 500 });
    }
};
