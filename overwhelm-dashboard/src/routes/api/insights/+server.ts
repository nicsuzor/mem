import { json } from '@sveltejs/kit';
import { readdir, readFile } from 'fs/promises';
import { join } from 'path';
import { env } from '$env/dynamic/private';

const AOPS_SESSIONS = env.AOPS_SESSIONS || process.env.AOPS_SESSIONS || '';
const SUMMARIES_DIR = AOPS_SESSIONS ? join(AOPS_SESSIONS, 'summaries') : '';

export async function GET() {
    if (!SUMMARIES_DIR) return json({ error: "AOPS_SESSIONS not set" }, { status: 500 });

    try {
        const files = await readdir(SUMMARIES_DIR);
        // Only load files after 20260429, which have the new stable JSON structure
        const recentFiles = files.filter(f => f.endsWith('.json') && f >= '20260429');

        const insights = [];

        for (const file of recentFiles) {
            try {
                const content = await readFile(join(SUMMARIES_DIR, file), 'utf-8');
                const data = JSON.parse(content);

                if (!data.token_metrics) continue;

                const date = data.date || file.slice(0, 8);
                const project = data.project || data.repo || 'unknown';
                const taskId = data.task_id || 'unknown';
                const provider = data.provider || 'unknown';
                const duration = data.token_metrics?.efficiency?.session_duration_minutes || 0;
                
                let prUrl = data.pr_url || null;
                if (!prUrl && data.timeline_events) {
                    for (const evt of data.timeline_events) {
                        const m = (evt.description || '').match(/https:\/\/github\.com\/[\w.-]+\/[\w.-]+\/pull\/\d+/);
                        if (m) {
                            prUrl = m[0];
                            break;
                        }
                    }
                }
                const pr = prUrl || 'none';

                const byAgent = data.token_metrics?.by_agent || {};
                
                for (const [agentName, metrics] of Object.entries(byAgent)) {
                    insights.push({
                        session_id: data.session_id,
                        date: date.slice(0, 10),
                        project,
                        task_id: taskId,
                        pr,
                        provider,
                        subagent: agentName,
                        duration_minutes: agentName === 'main' ? duration : 0,
                        input_tokens: (metrics as any).input || 0,
                        output_tokens: (metrics as any).output || 0,
                        cache_create_tokens: (metrics as any).cache_create || 0,
                        cache_read_tokens: (metrics as any).cache_read || 0,
                    });
                }
            } catch (e) {
                console.error(`Error reading ${file}`, e);
            }
        }
        
        return json({ insights });
    } catch (e: any) {
        return json({ error: e.message }, { status: 500 });
    }
}
