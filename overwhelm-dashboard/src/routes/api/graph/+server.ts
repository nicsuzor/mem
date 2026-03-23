import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { json } from '@sveltejs/kit';
import { env } from '$env/dynamic/private';
import type { RequestHandler } from './$types';

const AOPS_SESSIONS = env.AOPS_SESSIONS;

/** GET /api/graph — serve the single graph JSON from $AOPS_SESSIONS */
export const GET: RequestHandler = async () => {
    if (!AOPS_SESSIONS) {
        return json({ error: 'AOPS_SESSIONS environment variable is not set' }, { status: 503 });
    }

    const filepath = join(AOPS_SESSIONS, 'graph.json');

    try {
        const text = await readFile(filepath, 'utf-8');
        return new Response(text, {
            headers: { 'Content-Type': 'application/json' },
        });
    } catch {
        return json({ error: 'graph.json not found — run `pkb export` to generate' }, { status: 404 });
    }
};
