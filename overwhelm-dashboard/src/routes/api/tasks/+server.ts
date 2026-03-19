import { json } from '@sveltejs/kit';
import { listPkbTasks } from '$lib/server/pkb';
import type { RequestHandler } from './$types';

/** GET /api/tasks?status=ready&project=foo&limit=50 — list tasks via MCP */
export const GET: RequestHandler = async ({ url }) => {
    const status = url.searchParams.get('status') ?? undefined;
    const project = url.searchParams.get('project') ?? undefined;
    const assignee = url.searchParams.get('assignee') ?? undefined;
    const limit = parseInt(url.searchParams.get('limit') ?? '50', 10);

    const text = await listPkbTasks({ status, project, assignee, limit });
    if (text === null) {
        return json({ error: 'PKB unavailable' }, { status: 503 });
    }

    return json({ text });
};
