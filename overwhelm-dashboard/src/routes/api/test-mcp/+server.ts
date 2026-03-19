import { json } from '@sveltejs/kit';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import type { RequestHandler } from './$types';

export const GET: RequestHandler = async () => {
    const start = Date.now();

    const pkbRoot = process.env.ACA_DATA || '/opt/nic/brain';

    const transport = new StdioClientTransport({
        command: 'pkb',
        args: [pkbRoot],
    });

    const client = new Client({ name: 'overwhelm-dashboard-test', version: '1.0.0' });

    try {
        await client.connect(transport);
        const elapsed_connect = Date.now() - start;

        const result = await client.callTool({ name: 'list_tasks', arguments: { limit: 5 } });
        const elapsed_total = Date.now() - start;

        await client.close();

        return json({
            ok: true,
            connect_ms: elapsed_connect,
            total_ms: elapsed_total,
            tasks: result.content,
        });
    } catch (e: any) {
        try { await client.close(); } catch { /* ignore */ }
        return json({ ok: false, error: String(e?.message || e) }, { status: 500 });
    }
};
