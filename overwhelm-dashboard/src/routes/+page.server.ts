import { readFile, readdir, stat } from 'node:fs/promises';
import { join } from 'node:path';
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

function extractCleanPrompt(timeline: any[]): string {
    if (!timeline || !Array.isArray(timeline)) return '';
    for (const evt of timeline) {
        if (evt.type !== 'user_prompt') continue;
        const desc = (evt.description || '').trim();
        if (!desc) continue;
        
        // Skip bash I/O noise and slash commands
        if (desc.includes('<bash-input>') || desc.includes('<bash-stdout>') || /^\/\w/.test(desc)) continue;
        
        // Skip framework/automation noise
        if (desc.includes('▶ Task bound') || desc.includes('Compliance report ready')) continue;
        
        // Clean up scheduled task preamble
        if (desc.includes('<scheduled-task')) {
            return 'Scheduled: ' + (desc.match(/name="([^"]+)"/)?.[1] || 'task');
        }
        
        return desc;
    }
    return '';
}

function buildDailyStory(summaries: any[]): any {
    const story: string[] = [];
    const cutoff = Date.now() - 24 * 3600 * 1000;
    
    for (const s of summaries) {
        const stTime = s.date ? new Date(s.date).getTime() : 0;
        if (stTime < cutoff) continue;
        
        const proj = s.project || 'unknown';
        const accomplishments = s.accomplishments || [];
        
        if (accomplishments.length > 0) {
            for (const acc of accomplishments) {
                story.push(`[${proj}] ${acc}`);
            }
        } else if (s.summary) {
            story.push(`[${proj}] ${s.summary}`);
        } else if (s.timeline_events) {
            const cleanPrompt = extractCleanPrompt(s.timeline_events);
            if (cleanPrompt) {
                // Shorten description if it's too long
                const desc = cleanPrompt.slice(0, 150) + (cleanPrompt.length > 150 ? '...' : '');
                story.push(`[${proj}] ${desc}`);
            }
        }
    }
    return story.length > 0 ? { story } : null;
}

interface ProjectsConfig {
    pseudo_projects: string[];
}

async function loadProjectsConfig(): Promise<ProjectsConfig> {
    const empty: ProjectsConfig = { pseudo_projects: [] };
    if (!AOPS_SESSIONS) return empty;
    const data = await readJson(join(AOPS_SESSIONS, 'projects.json'));
    if (!data) return empty;
    return {
        pseudo_projects: Array.isArray(data.pseudo_projects) ? data.pseudo_projects : [],
    };
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
        const isGha = stem.includes('gha');
        const sessionType: 'polecat' | 'crew' | 'scheduled' | 'gha' | 'interactive' =
            isPolecat ? 'polecat' : isCrew ? 'crew' : isScheduled ? 'scheduled' : isGha ? 'gha' : 'interactive';

        // Extract the first meaningful user prompt from timeline as description
        // This is the best context for "what was this session about?"
        const timeline: any[] = data.timeline_events || [];
        const cleanPrompt = extractCleanPrompt(timeline);
        let description = data.summary || (cleanPrompt ? cleanPrompt.slice(0, 300) : '');
        
        if (!description) {
            // Fallback for sessions with no meaningful prompt
            if (sessionType === 'polecat') description = 'Autonomous Polecat worker';
            else if (sessionType === 'crew') description = 'Crew swarm session';
            else if (sessionType === 'scheduled') description = 'Scheduled cron task';
            else if (sessionType === 'gha') description = 'GitHub Actions workflow';
            else if (timeline.length > 0 && timeline[0].description) {
                // If it's an interactive session but has only noise, show truncated raw noise
                description = timeline[0].description.slice(0, 80).replace(/\n/g, ' ') + '...';
            } else {
                description = 'Active session (no description)';
            }
        }

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
            id: data.session_id || stem,
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

export const load = async () => {
    const [sessions, summaries, projectsConfig] = await Promise.all([
        findActiveSessions(48), // Fetch 48h to populate stale bucket
        loadRecentSummaries(3),
        loadProjectsConfig(),
    ]);

    // Bucket sessions by recency
    const activeSessions = sessions.filter(s => s.bucket === 'active');
    const pausedSessions = sessions.filter(s => s.bucket === 'paused');
    const staleSessions = sessions.filter(s => s.bucket === 'stale');
    const needsYouSessions = sessions.filter(s => s.needs_you);

    // Build project-level data: prefer graph data (via client), synthesis enriches
    // Collect all projects from sessions + synthesis
    const projectSet = new Set<string>();
    const projectLatestSession = new Map<string, number>();

    const pseudoProjects = new Set(projectsConfig.pseudo_projects);

    sessions.forEach(s => {
        if (s.project && !pseudoProjects.has(s.project)) {
            projectSet.add(s.project);
            const currentLatest = projectLatestSession.get(s.project) || 0;
            if (s.last_modified > currentLatest) {
                projectLatestSession.set(s.project, s.last_modified);
            }
        }
    });

    // Removed synthesis session fallback

    const projectProjects = Array.from(projectSet).sort((a, b) => {
        const aLatest = projectLatestSession.get(a) || 0;
        const bLatest = projectLatestSession.get(b) || 0;
        return bLatest - aLatest; // Sort by most recent session timestamp
    });

    const projectData: any = { meta: {}, tasks: {}, accomplishments: {}, sessions: {} };
    for (const proj of projectProjects) {
        projectData.meta[proj] = {
            latest_session: projectLatestSession.get(proj) || 0
        };
        projectData.tasks[proj] = [];
        projectData.sessions[proj] = sessions.filter(s => s.project === proj);
        const projAccomplishments: any[] = [];
        const seen = new Set<string>();
        for (const s of summaries) {
            if (s.project !== proj) continue;
            const accs = s.accomplishments || [];
            if (accs.length > 0) {
                for (const text of accs) {
                    if (!seen.has(text)) {
                        seen.add(text);
                        projAccomplishments.push({ description: text });
                    }
                }
            } else if (s.summary) {
                if (!seen.has(s.summary)) {
                    seen.add(s.summary);
                    projAccomplishments.push({ description: s.summary });
                }
            }
        }
        projectData.accomplishments[proj] = projAccomplishments;
    }

    // Pipeline health — fail fast and loud when data sources are missing
    const summariesDirOk = AOPS_SESSIONS !== '';
    const dailyStory = buildDailyStory(summaries);

    return {
        dashboardData: {
            pipeline_errors: [
                ...(!summariesDirOk ? ['$AOPS_SESSIONS not set — session discovery disabled'] : []),
            ],
            // Bucketed sessions for triage display
            active_agents: activeSessions,
            paused_sessions: pausedSessions,
            stale_sessions: staleSessions,
            needs_you: needsYouSessions,
            synthesis: null,
            daily_story: dailyStory,
            
            project_projects: projectProjects,
            project_data: projectData,
            path: buildPathData(summaries),
            projects_config: projectsConfig,
        },
    };
};
