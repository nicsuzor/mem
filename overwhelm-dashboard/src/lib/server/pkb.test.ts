/**
 * Integration tests for PKB MCP HTTP server connection.
 *
 * Requires a running `pkb mcp --http` server.
 * Set PKB_MCP_URL env var or defaults to http://127.0.0.1:8026/mcp.
 */

import { describe, it, expect, beforeAll } from 'vitest';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';

const PKB_MCP_URL = process.env.PKB_MCP_URL || 'http://127.0.0.1:8026/mcp';

let client: Client;
let serverAvailable = false;

beforeAll(async () => {
    const transport = new StreamableHTTPClientTransport(new URL(PKB_MCP_URL));
    client = new Client({ name: 'pkb-test', version: '1.0.0' });
    try {
        await client.connect(transport);
        serverAvailable = true;
    } catch (error) {
        console.warn(`PKB server not available at ${PKB_MCP_URL} — skipping integration tests: ${error}`);
        console.warn(`PKB server not available at ${PKB_MCP_URL} — skipping integration tests`);
    }
}, 15_000);

function requireServer() {
    if (!serverAvailable) {
        throw new Error(`PKB server not available at ${PKB_MCP_URL}`);
    }
}

describe('PKB HTTP MCP server', () => {
    it('connects and initializes', () => {
        requireServer();
        expect(client).toBeDefined();
    });

    it('lists tools', async () => {
        requireServer();
        const result = await client.listTools();
        const names = result.tools.map((t) => t.name);
        expect(names).toContain('search');
        expect(names).toContain('list_tasks');
        expect(names).toContain('get_task');
        expect(names).toContain('task_summary');
        expect(names.length).toBeGreaterThanOrEqual(26);
    });

    it('search returns results', async () => {
        requireServer();
        const result = await client.callTool({
            name: 'search',
            arguments: { query: 'test', limit: 3 },
        });
        const text = extractText(result);
        expect(text).toContain('Found');
        expect(text).toContain('results');
    });

    it('task_summary returns stats', async () => {
        requireServer();
        const result = await client.callTool({
            name: 'task_summary',
            arguments: {},
        });
        const text = extractText(result);
        const stats = JSON.parse(text);
        expect(stats).toHaveProperty('ready');
        expect(stats).toHaveProperty('blocked');
        expect(stats.ready).toBeGreaterThan(0);
    });

    it('list_tasks returns tasks', async () => {
        requireServer();
        const result = await client.callTool({
            name: 'list_tasks',
            arguments: { status: 'ready', limit: 5 },
        });
        const text = extractText(result);
        expect(text.length).toBeGreaterThan(0);
    });

    it('graph_stats returns stats', async () => {
        requireServer();
        const result = await client.callTool({
            name: 'graph_stats',
            arguments: {},
        });
        const text = extractText(result);
        const stats = JSON.parse(text);
        expect(stats).toHaveProperty('total_tasks');
        expect(stats.total_tasks).toBeGreaterThan(0);
    });

    it('search with unknown query returns gracefully', async () => {
        requireServer();
        const result = await client.callTool({
            name: 'search',
            arguments: { query: 'xyzzy_nonexistent_term_12345', limit: 3 },
        });
        const text = extractText(result);
        // Should still return a valid response, even if 0 results
        expect(text).toBeDefined();
    });

    // AC 2 from task-491ce1f9: dashboard write actions must reliably reach the
    // PKB MCP server and the change must be visible on reload. Exercises the
    // same get_task / update_task call surface used by /api/task/status.
    //
    // Mutate-and-restore pattern: picks an existing task, adds a unique
    // marker tag, asserts the marker round-trips through update -> get,
    // then restores the original tags. Non-destructive.
    it('round-trip: update_task -> get_task observes the change', async () => {
        requireServer();

        // Find any existing task. Skip when the PKB is empty (sandbox case).
        const listText = extractText(
            await client.callTool({
                name: 'list_tasks',
                arguments: { limit: 1 },
            }),
        );
        const idMatch = listText.match(/\b((?:task|aops|epic|framework|spike|polecat)-[0-9a-f]{6,})\b/);
        if (!idMatch) {
            console.warn('round-trip test: no existing tasks in PKB — skipping');
            return;
        }
        const taskId = idMatch[1];
        const marker = `roundtrip-${Date.now()}`;

        const beforeJson = JSON.parse(
            extractText(await client.callTool({ name: 'get_task', arguments: { id: taskId } })),
        );
        const originalTags: string[] = Array.isArray(beforeJson.frontmatter?.tags)
            ? [...beforeJson.frontmatter.tags]
            : [];

        try {
            const updateResult = await client.callTool({
                name: 'update_task',
                arguments: {
                    id: taskId,
                    updates: { tags: [...originalTags, marker] },
                },
            });
            const updateText = extractText(updateResult);
            expect(updateText.toLowerCase()).not.toContain('error');

            const afterJson = JSON.parse(
                extractText(await client.callTool({ name: 'get_task', arguments: { id: taskId } })),
            );
            expect(afterJson.frontmatter?.tags).toContain(marker);
        } finally {
            // Restore even if assertion above failed.
            await client
                .callTool({
                    name: 'update_task',
                    arguments: { id: taskId, updates: { tags: originalTags } },
                })
                .catch(() => {});
        }
    }, 20_000);
});

function extractText(result: Awaited<ReturnType<Client['callTool']>>): string {
    const content = result.content;
    if (Array.isArray(content) && content.length > 0) {
        const first = content[0];
        if (first && typeof first === 'object' && 'text' in first) {
            return String((first as { text: unknown }).text);
        }
    }
    return '';
}
