/**
 * PKB client singleton — single MCP connection to pkb binary, shared across all server routes.
 *
 * Usage:
 *   import { callPkbTool, getPkbTask } from '$lib/server/pkb';
 *
 *   const tasks = await callPkbTool('list_tasks', { status: 'ready', limit: 10 });
 *   const task  = await getPkbTask('some-task-id');
 */

import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface PkbTaskFrontmatter {
    id: string;
    title: string;
    status?: string;
    priority?: number;
    assignee?: string;
    project?: string;
    parent?: string;
    type?: string;
    tags?: string[];
    depends_on?: string[];
    [key: string]: unknown;
}

export interface PkbTaskRef {
    id: string;
    title: string;
    status?: string;
}

export interface PkbTask {
    frontmatter: PkbTaskFrontmatter;
    body: string;
    path: string;
    depends_on: PkbTaskRef[];
    blocks: PkbTaskRef[];
    children: PkbTaskRef[];
    parent: PkbTaskRef | null;
    downstream_weight: number;
    stakeholder_exposure: boolean;
}

// ---------------------------------------------------------------------------
// Singleton state
// ---------------------------------------------------------------------------

let client: Client | null = null;
let initPromise: Promise<Client | null> | null = null;

const PKB_ROOT = process.env.ACA_DATA || '/opt/nic/brain';

async function connect(): Promise<Client | null> {
    const transport = new StdioClientTransport({
        command: 'pkb',
        args: [PKB_ROOT],
    });

    const c = new Client({ name: 'overwhelm-dashboard', version: '1.0.0' });

    // Reset singleton when the connection closes
    transport.onclose = () => {
        client = null;
        initPromise = null;
    };

    try {
        await c.connect(transport);
        return c;
    } catch (err) {
        client = null;
        initPromise = null;
        console.error('[pkb] Failed to connect to pkb binary:', err);
        return null;
    }
}

/** Returns the shared client, initializing it on first call. Thread-safe via Promise deduplication. */
async function getClient(): Promise<Client | null> {
    if (client) return client;

    if (!initPromise) {
        initPromise = connect().then((c) => {
            client = c;
            initPromise = null;
            return c;
        });
    }

    return initPromise;
}

// ---------------------------------------------------------------------------
// Core API
// ---------------------------------------------------------------------------

/** Call any pkb MCP tool by name. Returns the raw text content string, or null on error. */
export async function callPkbTool(
    name: string,
    args: Record<string, unknown> = {}
): Promise<string | null> {
    const c = await getClient();
    if (!c) return null;

    try {
        const result = await c.callTool({ name, arguments: args });
        const content = result.content;
        if (Array.isArray(content) && content.length > 0) {
            const first = content[0];
            if (first && typeof first === 'object' && 'text' in first) {
                return String((first as { text: unknown }).text);
            }
        }
        return null;
    } catch (err) {
        // Connection may be broken — reset so next call reconnects
        client = null;
        initPromise = null;
        console.error(`[pkb] Tool call failed (${name}):`, err);
        return null;
    }
}

// ---------------------------------------------------------------------------
// Typed convenience wrappers
// ---------------------------------------------------------------------------

/** Fetch a single task by ID. Returns parsed task object or null. */
export async function getPkbTask(id: string): Promise<PkbTask | null> {
    const text = await callPkbTool('get_task', { id });
    if (!text) return null;
    try {
        return JSON.parse(text) as PkbTask;
    } catch {
        return null;
    }
}

/** Update frontmatter fields on a task. Returns true on success. */
export async function updatePkbTask(
    id: string,
    updates: Record<string, unknown>
): Promise<boolean> {
    const text = await callPkbTool('update_task', { id, updates });
    return text !== null && !text.startsWith('Error');
}

/** Create a new task. Returns the raw confirmation text (includes new task ID) or null. */
export async function createPkbTask(args: Record<string, unknown>): Promise<string | null> {
    return callPkbTool('create_task', args);
}

/** Semantic search over tasks. Returns raw markdown text or null. */
export async function searchPkbTasks(
    query: string,
    opts: { limit?: number; project?: string } = {}
): Promise<string | null> {
    return callPkbTool('task_search', { query, ...opts });
}

/** List tasks with optional filters. Returns raw markdown table text or null. */
export async function listPkbTasks(opts: {
    status?: string;
    project?: string;
    assignee?: string;
    priority?: number;
    limit?: number;
} = {}): Promise<string | null> {
    return callPkbTool('list_tasks', opts);
}
