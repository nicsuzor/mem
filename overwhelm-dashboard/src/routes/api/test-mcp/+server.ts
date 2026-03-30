import { json } from '@sveltejs/kit';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import type { RequestHandler } from './$types';

const PKB_MCP_URL = process.env.PKB_MCP_URL || 'http://127.0.0.1:8026/mcp';

export const GET: RequestHandler = async () => {
    const start = Date.now();

    const transport = new StreamableHTTPClientTransport(
        new URL(PKB_MCP_URL),
    );

    const client = new Client({ name: 'overwhelm-dashboard-test', version: '1.0.0' });

    try {
        await client.connect(transport);
        const elapsed_connect = Date.now() - start;

        const result = await client.callTool({ name: 'list_tasks', arguments: { limit: 5 } });
        const elapsed_total = Date.now() - start;

        await client.close();

        return json({
            ok: true,
            url: PKB_MCP_URL,
            connect_ms: elapsed_connect,
            total_ms: elapsed_total,
            tasks: result.content,
        });
    } catch (e: any) {
        try { await client.close(); } catch { /* ignore */ }
        return json({ ok: false, url: PKB_MCP_URL, error: String(e?.message || e) }, { status: 500 });
    }
};
