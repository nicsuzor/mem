import { json } from '@sveltejs/kit';
import { updatePkbTask } from '$lib/server/pkb';
import type { RequestHandler } from './$types';

/** POST /api/task/status — update task fields via MCP (replaces CLI spawn) */
export const POST: RequestHandler = async ({ request }) => {
    const body = await request.json().catch(() => null);
    const { id, status, priority, assignee, parent, refile } = body ?? {};

    if (!id) {
        return json({ error: 'Missing id' }, { status: 400 });
    }

    // Build updates object from whichever fields were provided
    const updates: Record<string, unknown> = {};
    if (status !== undefined) updates.status = status;
    if (priority !== undefined) updates.priority = priority;
    if (assignee !== undefined) updates.assignee = assignee;
    if (parent !== undefined) updates.parent = parent;
    if (refile !== undefined) updates.refile = refile;

    // PKB requires completion_evidence when setting status to done
    if (status === 'done') {
        updates.completion_evidence = 'Manually completed via dashboard';
    }

    if (Object.keys(updates).length === 0) {
        return json({ error: 'No updates provided' }, { status: 400 });
    }

    const ok = await updatePkbTask(id, updates);
    if (!ok) {
        return json({ error: 'Failed to update task — PKB unavailable or task not found' }, { status: 500 });
    }

    return json({ success: true, message: `Task updated: ${id}` });
};
