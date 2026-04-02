import { json } from '@sveltejs/kit';
import { getPkbTask } from '$lib/server/pkb';
import type { RequestHandler } from './$types';

/** GET /api/task?id=<task-id> — fetch structured task data via MCP */
export const GET: RequestHandler = async ({ url }) => {
    const id = url.searchParams.get('id');
    if (!id) {
        return json({ error: 'Missing id parameter' }, { status: 400 });
    }

    const task = await getPkbTask(id);
    if (task === null) {
        return json({ error: 'Task not found or PKB unavailable' }, { status: 503 });
    }

    return json(task);
};
