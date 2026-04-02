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

async function findActiveSessions(hours = 4): Promise<any[]> {
    const claudeProjects = join(env.HOME || os.homedir(), '.claude', 'projects');
    const cutoff = Date.now() - hours * 3600 * 1000;
    const results: any[] = [];

    let projectDirs: string[];
    try {
        projectDirs = await readdir(claudeProjects);
    } catch {
        return results;
    }

    for (const projName of projectDirs) {
        if (projName.includes('-tmp') || projName.includes('-var-folders') || projName.endsWith('-hooks')) continue;

        const projPath = join(claudeProjects, projName);
        let sessionDirs: string[];
        try {
            sessionDirs = await readdir(projPath);
        } catch {
            continue;
        }

        for (const sessName of sessionDirs) {
            if (!/^\d/.test(sessName)) continue;
            const stateFile = join(projPath, sessName, 'session-state.json');
            let st;
            try {
                st = await stat(stateFile);
            } catch {
                continue;
            }
            if (st.mtimeMs < cutoff) continue;

            const stateData = await readJson(stateFile);
            if (!stateData) continue;

            const insights = stateData.insights || {};
            const project = insights.project || stateData.project || formatProjectName(projName);
            const minutesAgo = (Date.now() - st.mtimeMs) / 60000;

            // Determine session status for badges
            const hoursAgo = minutesAgo / 60;
            let bucket: 'active' | 'paused' | 'stale';
            if (hoursAgo < 4) bucket = 'active';
            else if (hoursAgo < 24) bucket = 'paused';
            else bucket = 'stale';

            // Detect "needs you" — errored, waiting for input, or completed awaiting review
            const sessionStatus = stateData.status || insights.status || '';
            const hasError = sessionStatus === 'error' || sessionStatus === 'errored' || !!stateData.error;
            const waitingForInput = sessionStatus === 'waiting' || sessionStatus === 'needs_input';
            const completedAwaitingReview = (sessionStatus === 'completed' || sessionStatus === 'done') && minutesAgo < 240;
            const needsYou = hasError || waitingForInput || completedAwaitingReview;

            let statusBadge: 'running' | 'needs_you' | 'errored' | 'completed' | 'paused' | 'idle';
            if (hasError) statusBadge = 'errored';
            else if (waitingForInput) statusBadge = 'needs_you';
            else if (minutesAgo < 10) statusBadge = 'running';
            else if (completedAwaitingReview) statusBadge = 'completed';
            else if (bucket === 'paused') statusBadge = 'paused';
            else statusBadge = 'idle';

            results.push({
                session_id: stateData.session_id || sessName.split('-').pop(),
                project,
                description: insights.summary || stateData.summary || stateData.current_task || '',
                goal: insights.goal || '',
                started_at: new Date(st.mtimeMs - (stateData.duration_ms || 0)).toISOString(),
                time_display: minutesAgo < 60 ? `${Math.round(minutesAgo)}m ago` : `${Math.round(minutesAgo / 60)}h ago`,
                now_task: insights.current_task || stateData.current_task || '',
                next_task: insights.next_task || '',
                progress_done: insights.progress_done ?? 0,
                progress_total: insights.progress_total ?? 0,
                outcome_text: insights.outcome || '',
                is_active: minutesAgo < 10,
                last_modified: st.mtimeMs,
                bucket,
                status_badge: statusBadge,
                needs_you: needsYou,
            });
        }
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

    return {
        dashboardData: {
            // Bucketed sessions for triage display
            active_agents: activeSessions,
            paused_sessions: pausedSessions,
            stale_sessions: staleSessions,
            needs_you: needsYouSessions,
            synthesis: synthesis ? {
                alignment: synthesis.alignment,
                recent_context: synthesis.context?.recent_threads?.join(', ') || '',
                blockers: synthesis.waiting_on?.length ? synthesis.waiting_on : null,
                _age_minutes: synthesis._age_minutes,
                sessions: synthesis.sessions,
                narrative: synthesis.narrative,
                daily_narrative: synthesis.daily_narrative,
            } : null,
            // Prefer LLM-generated daily_narrative from /daily skill; fall back to mechanical narrative
            daily_story: synthesis?.daily_narrative ? { story: synthesis.daily_narrative }
                : synthesis?.narrative ? { story: synthesis.narrative }
                : null,
            
            project_projects: projectProjects,
            project_data: projectData,
            path: buildPathData(summaries),
        },
    };
};
