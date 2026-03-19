import { json } from '@sveltejs/kit';
import { env } from '$env/dynamic/private';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import type { RequestHandler } from './$types';

const execFileAsync = promisify(execFile);

const AOPS_BIN = env.AOPS_BIN || 'aops';
const ACA_DATA = env.ACA_DATA || '';

export const POST: RequestHandler = async ({ request }) => {
    const body = await request.json().catch(() => null);
    const { id, status } = body ?? {};

    if (!id || !status) {
        return json({ error: 'Missing id or status' }, { status: 400 });
    }
    // Only allow safe characters (task IDs and status values are alphanumeric + hyphens)
    if (!/^[\w-]+$/.test(id) || !/^[\w-]+$/.test(status)) {
        return json({ error: 'Invalid id or status' }, { status: 400 });
    }

    if (!ACA_DATA) {
        return json({ error: 'ACA_DATA environment variable is not set' }, { status: 503 });
    }

    const rootArgs = ['--pkb-root', ACA_DATA];

    const args: string[] =
        status === 'done'
            ? [...rootArgs, 'done', id]
            : [...rootArgs, 'update', id, '--status', status];

    try {
        const { stdout } = await execFileAsync(AOPS_BIN, args, { timeout: 15_000 });
        return json({ success: true, message: stdout.trim() });
    } catch (e: any) {
        const msg = e.stderr?.trim() || e.message || 'aops command failed';
        return json({ error: msg }, { status: 500 });
    }
};
