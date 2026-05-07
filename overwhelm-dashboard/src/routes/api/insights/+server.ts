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
                
                // Timeline events text for heuristics
                let fullText = '';
                if (data.timeline_events) {
                    fullText = data.timeline_events.map((e: any) => e.description || '').join(' ');
                }

                // PR extraction
                let prUrl = data.pr_url || null;
                if (!prUrl) {
                    const m = fullText.match(/https:\/\/github\.com\/[\w.-]+\/[\w.-]+\/pull\/\d+/);
                    if (m) prUrl = m[0];
                }
                const pr = prUrl || 'none';

                // Project extraction
                let project = data.project || data.repo;
                if (!project || /^[0-9a-f]{6,}$/i.test(project)) {
                    // Try to recover from PR URL
                    if (prUrl) {
                        const prMatch = prUrl.match(/github\.com\/[\w.-]+\/([\w.-]+)/i);
                        if (prMatch) project = prMatch[1];
                    }
                    // Try to recover from filename e.g., 20260501-1149-hash-project-slug.json
                    if (!project || /^[0-9a-f]{6,}$/i.test(project)) {
                        const parts = file.split('-');
                        if (parts.length >= 4) {
                            project = parts[3];
                        }
                    }
                }
                project = project || 'unknown';

                // Task ID extraction
                let taskId = data.task_id;
                if (!taskId || taskId === 'unknown') {
                    const taskMatch = fullText.match(/\b([a-zA-Z]+-[0-9a-f]{4,})\b/i);
                    if (taskMatch) taskId = taskMatch[1];
                }
                taskId = taskId || 'unknown';

                let startedAt = data.started_at || data.started || data.date;
                if (!startedAt || !startedAt.includes('T')) {
                    const m = file.match(/^(\d{4})(\d{2})(\d{2})-(\d{2})(\d{2})/);
                    if (m) {
                        startedAt = `${m[1]}-${m[2]}-${m[3]}T${m[4]}:${m[5]}:00`;
                    } else {
                        startedAt = date;
                    }
                }
                let slug = '';
                const fileParts = file.replace('.json', '').split('-');
                const sidIndex = fileParts.indexOf(data.session_id);
                if (sidIndex !== -1 && sidIndex < fileParts.length - 1) {
                    slug = fileParts.slice(sidIndex + 1).join(' ');
                    slug = slug.charAt(0).toUpperCase() + slug.slice(1);
                }
                
                let description = data.description || data.summary || data.outcome || slug || '';
                if (description.startsWith('Session in') && description.endsWith('completed.')) {
                    description = slug || description;
                }

                const provider = data.provider || 'unknown';
                const client = data.client || 'unknown';
                const surface = data.surface || 'unknown';
                const duration = data.token_metrics?.efficiency?.session_duration_minutes || 0;

                const byAgent = data.token_metrics?.by_agent || {};
                
                for (const [agentName, metrics] of Object.entries(byAgent)) {
                    insights.push({
                        session_id: data.session_id,
                        date: date.slice(0, 10),
                        started_at: startedAt,
                        description: description,
                        project,
                        task_id: taskId,
                        pr,
                        provider,
                        client,
                        surface,
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
