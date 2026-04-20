import { readFile, readdir, stat } from 'node:fs/promises';
import { join } from 'node:path';
import os from 'node:os';
import { env } from '$env/dynamic/private';

const AOPS_SESSIONS = env.AOPS_SESSIONS || '';
const SUMMARIES_DIR = AOPS_SESSIONS ? join(AOPS_SESSIONS, 'summaries') : '';

async function readJson(path: string): Promise<any | null> {
    try {
        const text = await readFile(path, 'utf-8');
        return JSON.parse(text);
    } catch {
        return null;
    }
}

async function loadSynthesis(): Promise<any | null> {
    if (!AOPS_SESSIONS) return null;
    const path = join(AOPS_SESSIONS, 'synthesis.json');
    const data = await readJson(path);
    if (!data) return null;

    try {
        const st = await stat(path);
        data._age_minutes = (Date.now() - st.mtimeMs) / 60000;
    } catch { /* ignore */ }

    return data;
}

/**
 * Find active sessions from $AOPS_SESSIONS/summaries/.
 * A session is "current" if: it has a summary file, no outcome (not finished via /dump),
 * and was written to in the last `hours` hours.
 */
async function findActiveSessions(hours = 4): Promise<any[]> {
    if (!AOPS_SESSIONS) return [];
    const summariesDir = join(AOPS_SESSIONS, 'summaries');
    const cutoff = Date.now() - hours * 3600 * 1000;
    const results: any[] = [];

    let files: string[];
    try {
        files = await readdir(summariesDir);
    } catch {
        return results;
    }

    // Generate prefixes for today, yesterday, and the day before to handle 48h cutoff and local timezones
    const prefixes = [0, 1, 2, 3].map(days => {
        const d = new Date(Date.now() - days * 86400000);
        const y = d.getFullYear();
        const m = String(d.getMonth() + 1).padStart(2, '0');
        const day = String(d.getDate()).padStart(2, '0');
        return `${y}${m}${day}`;
    });
    const candidates = files.filter(f => f.endsWith('.json') && prefixes.some(p => f.startsWith(p)));

    for (const filename of candidates) {
        const filePath = join(summariesDir, filename);
        let st;
        try {
            st = await stat(filePath);
        } catch {
            continue;
        }
        if (st.mtimeMs < cutoff) continue;

        const data = await readJson(filePath);
        if (!data) continue;

        // Sessions with an outcome have finished — skip them
        if (data.outcome) continue;

        const project = data.project || 'unknown';
        const minutesAgo = (Date.now() - st.mtimeMs) / 60000;
        const hoursAgo = minutesAgo / 60;

        // Classify session type from filename
        const stem = filename.replace('.json', '');
        const isPolecat = stem.includes('polecat');
        const isCrew = stem.includes('crew');
        const isScheduled = stem.includes('scheduled');
        const sessionType: 'polecat' | 'crew' | 'scheduled' | 'interactive' =
            isPolecat ? 'polecat' : isCrew ? 'crew' : isScheduled ? 'scheduled' : 'interactive';

        // Extract the first meaningful user prompt from timeline as description
        // This is the best context for "what was this session about?"
        const timeline: any[] = data.timeline_events || [];
        let firstPrompt = '';
        for (const evt of timeline) {
            if (evt.type !== 'user_prompt') continue;
            const desc = evt.description || '';
            // Skip bash I/O noise and slash commands
            if (desc.includes('<bash-input>') || desc.includes('<bash-stdout>') || /^\/\w/.test(desc)) continue;
            // Clean up scheduled task preamble
            if (desc.includes('<scheduled-task')) {
                firstPrompt = 'Scheduled: ' + (desc.match(/name="([^"]+)"/)?.[1] || 'task');
                break;
            }
            firstPrompt = desc.slice(0, 300);
            break;
        }
        const description = data.summary || firstPrompt || '';

        // All clean user prompts for expand view
        const allPrompts: string[] = [];
        for (const evt of timeline) {
            if (evt.type !== 'user_prompt') continue;
            const desc = evt.description || '';
            if (desc.includes('<bash-input>') || desc.includes('<bash-stdout>') || /^\/\w/.test(desc)) continue;
            if (desc.includes('<scheduled-task')) continue;
            allPrompts.push(desc);
        }

        // Duration: use time since first event (the cron snapshot of token_metrics is stale for running sessions)
        const firstEvent = timeline[0]?.timestamp;
        const durationMin = firstEvent
            ? (Date.now() - new Date(firstEvent).getTime()) / 60000
            : data.token_metrics?.efficiency?.session_duration_minutes;
        // Prompt count: only what the cron captured — may be incomplete for running sessions
        const promptCount = timeline.filter((e: any) => e.type === 'user_prompt').length;

        let bucket: 'active' | 'paused' | 'stale';
        if (hoursAgo < 4) bucket = 'active';
        else if (hoursAgo < 24) bucket = 'paused';
        else bucket = 'stale';

        let statusBadge: string;
        if (minutesAgo < 10) statusBadge = 'running';
        else if (hoursAgo < 1) statusBadge = 'idle';
        else statusBadge = 'paused';

        results.push({
            session_id: data.session_id || '',
            project,
            description,
            session_type: sessionType,
            started_at: data.date || new Date(st.mtimeMs).toISOString(),
            time_display: minutesAgo < 60 ? `${Math.round(minutesAgo)}m ago` : `${Math.round(hoursAgo)}h ago`,
            duration_min: durationMin,
            prompt_count: promptCount,
            prompts: allPrompts,
            is_active: minutesAgo < 10,
            last_modified: st.mtimeMs,
            bucket,
            status_badge: statusBadge,
            needs_you: false,
            source: 'summaries',
        });
    }

    results.sort((a, b) => b.last_modified - a.last_modified);
    return results;
}

async function loadRecentSummaries(days = 3): Promise<any[]> {
    if (!SUMMARIES_DIR) return [];

    let files: string[];
    try {
        files = await readdir(SUMMARIES_DIR);
    } catch {
        return [];
    }

    // Build date prefixes for the last N days
    const prefixes: string[] = [];
    for (let i = 0; i < days; i++) {
        const d = new Date(Date.now() - i * 86400000);
        prefixes.push(d.toISOString().slice(0, 10).replace(/-/g, ''));
    }

    const jsonFiles = files
        .filter(f => f.endsWith('.json') && prefixes.some(p => f.startsWith(p)))
        .sort();

    const summaries: any[] = [];
    for (const f of jsonFiles) {
        const data = await readJson(join(SUMMARIES_DIR, f));
        if (data) {
            data._filename = f;
            summaries.push(data);
        }
    }
    return summaries;
}

function buildPathData(summaries: any[]): any {
    const abandoned: any[] = [];
    const now = new Date();
    const todayStr = now.toISOString().slice(0, 10);
    const yesterday = new Date(now.getTime() - 86400000);
    const yesterdayStr = yesterday.toISOString().slice(0, 10);

    // Collect accomplishments per project, deduped
    const byProject = new Map<string, { items: any[]; latestDate: string }>();
    const seenAccomplishments = new Set<string>();

    for (const s of summaries) {
        const proj = s.project || 'unknown';
        if (!byProject.has(proj)) byProject.set(proj, { items: [], latestDate: '' });
        const group = byProject.get(proj)!;

        const sessionDate = s.date || '';
        const dateStr = sessionDate.slice(0, 10);
        if (dateStr > group.latestDate) group.latestDate = dateStr;

        const outcome = s.outcome || 'unknown';

        // Use accomplishments as the primary source — meaningful outcomes, not raw events
        const accomplishments = s.accomplishments || [];
        if (accomplishments.length > 0) {
            for (const acc of accomplishments) {
                const key = `${proj}:${acc}`;
                if (seenAccomplishments.has(key)) continue;
                seenAccomplishments.add(key);
                group.items.push({ text: acc, outcome, date: dateStr });
            }
        } else if (s.summary) {
            const key = `${proj}:${s.summary}`;
            if (!seenAccomplishments.has(key)) {
                seenAccomplishments.add(key);
                group.items.push({ text: s.summary, outcome, date: dateStr });
            }
        }

        // Detect abandoned work: sessions with friction or no outcome
        if (outcome !== 'success' && s.friction_points?.length > 0) {
            for (const fp of s.friction_points) {
                const sessionDateObj = sessionDate ? new Date(sessionDate) : null;
                const minutesAgo = sessionDateObj ? (Date.now() - sessionDateObj.getTime()) / 60000 : 0;
                abandoned.push({
                    project: proj,
                    description: fp,
                    time_ago: minutesAgo < 60
                        ? `${Math.round(minutesAgo)}m ago`
                        : minutesAgo < 1440
                            ? `${Math.round(minutesAgo / 60)}h ago`
                            : `${Math.round(minutesAgo / 1440)}d ago`,
                });
            }
        }
    }

    // Build activity feed sorted by most recent activity
    const activity = Array.from(byProject.entries())
        .map(([project, { items, latestDate }]) => {
            let period: string;
            if (latestDate === todayStr) period = 'today';
            else if (latestDate === yesterdayStr) period = 'yesterday';
            else period = `${Math.ceil((now.getTime() - new Date(latestDate).getTime()) / 86400000)}d ago`;
            return { project, period, latestDate, items };
        })
        .filter(g => g.items.length > 0)
        .sort((a, b) => b.latestDate.localeCompare(a.latestDate));

    return { activity, abandoned_work: abandoned };
}

function formatProjectName(folder: string): string {
    const parts = folder.replace(/^-/, '').split('-');
    // Derive skip list from environment instead of hardcoding usernames
    const homeSegments = (env.HOME || os.homedir()).split('/').filter(Boolean);
    const skip = new Set([...homeSegments, 'src', 'opt', '_aops', '']);
    const meaningful = parts.filter(p => !skip.has(p) && !/^[a-f0-9]{8,}$/.test(p));
    return meaningful.pop() || folder;
}

export const load = async () => {
    const [synthesis, sessions, summaries] = await Promise.all([
        loadSynthesis(),
        findActiveSessions(48), // Fetch 48h to populate stale bucket
        loadRecentSummaries(3),
    ]);

    // Bucket sessions by recency
    const activeSessions = sessions.filter(s => s.bucket === 'active');
    const pausedSessions = sessions.filter(s => s.bucket === 'paused');
    const staleSessions = sessions.filter(s => s.bucket === 'stale');
    const needsYouSessions = sessions.filter(s => s.needs_you);

    // Build project-level data: prefer graph data (via client), synthesis enriches
    // Collect all projects from sessions + synthesis
    const projectSet = new Set<string>();
    sessions.forEach(s => { if (s.project) projectSet.add(s.project); });
    if (synthesis?.sessions?.by_project) {
        Object.keys(synthesis.sessions.by_project).forEach(p => projectSet.add(p));
    }
    const projectProjects = Array.from(projectSet).sort();

    const projectData: any = { meta: {}, tasks: {}, accomplishments: {}, sessions: {} };
    for (const proj of projectProjects) {
        projectData.meta[proj] = {};
        projectData.tasks[proj] = [];
        projectData.sessions[proj] = sessions.filter(s => s.project === proj);
        projectData.accomplishments[proj] = synthesis
            ? (synthesis.accomplishments?.items || [])
                .filter((a: any) => a.project === proj)
                .map((a: any) => ({ description: a.text }))
            : [];
    }

    // Pipeline health — fail fast and loud when data sources are missing
    const synthesisPipelineOk = synthesis !== null;
    const dailyStoryOk = synthesis?.narrative != null;
    const summariesDirOk = AOPS_SESSIONS !== '';

    return {
        dashboardData: {
            pipeline_errors: [
                ...(!summariesDirOk ? ['$AOPS_SESSIONS not set — session discovery disabled'] : []),
                ...(!synthesisPipelineOk ? ['synthesis.json not found or unreadable — is /daily running?'] : []),
                ...(synthesisPipelineOk && !dailyStoryOk ? ['synthesis.json has no narrative — run /daily to generate'] : []),
            ],
            // Bucketed sessions for triage display
            active_agents: activeSessions,
            paused_sessions: pausedSessions,
            stale_sessions: staleSessions,
            needs_you: needsYouSessions,
            synthesis: synthesis ? {
                _age_minutes: synthesis._age_minutes,
                sessions: synthesis.sessions,
            } : null,
            daily_story: synthesis?.narrative ? { story: synthesis.narrative } : null,
            
            project_projects: projectProjects,
            project_data: projectData,
            path: buildPathData(summaries),
        },
    };
};
