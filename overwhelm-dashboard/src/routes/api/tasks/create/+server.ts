import { json } from '@sveltejs/kit';
import { createPkbTask } from '$lib/server/pkb';
import type { RequestHandler } from './$types';

/** POST /api/tasks/create — create a new task via MCP */
export const POST: RequestHandler = async ({ request }) => {
    const body = await request.json().catch(() => null);
    const { title, project, priority } = body ?? {};

    if (!title?.trim()) {
        return json({ error: 'Missing title' }, { status: 400 });
    }

    const args: Record<string, unknown> = { title: title.trim() };
    if (project) args.project = project;
    if (priority !== undefined) args.priority = priority;

    const result = await createPkbTask(args);
    if (result === null) {
        return json({ error: 'Failed to create task — PKB unavailable' }, { status: 503 });
    }

    // Extract task ID from response text: pattern like `some-task-abc123ef`
    const match = result.match(/`([a-z][a-z0-9-]+-[a-f0-9]{8})`/);
    const id = match?.[1] ?? null;

    return json({ ok: true, id, message: result.trim() });
};
