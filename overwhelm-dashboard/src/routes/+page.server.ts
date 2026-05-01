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

// A hash-like project value is what the session writer produces when it can't determine
// the real project — typically a UUID fragment or git-worktree-name suffix gets stuffed
// into the field. Pure hex 6+ chars catches those; real project names ("aops", "academicOps",
// "gemini-02446f", "polecat-e2e-test-718304b2") all contain non-hex characters or separators.
function isHashLikeProject(name: string | null | undefined): boolean {
    if (!name) return false;
    return /^[0-9a-f]{6,}$/i.test(name);
}

// Best-effort recovery of the real project for a session whose project field was clobbered
// with a hash. Returns the canonical project name (preserving casing from the valid set) or null.
function recoverProject(data: any, validProjects: Set<string>): string | null {
    const parts: string[] = [];
    if (typeof data.summary === 'string') parts.push(data.summary);
    if (Array.isArray(data.accomplishments)) parts.push(...data.accomplishments.filter((s: any) => typeof s === 'string'));
    if (Array.isArray(data.friction_points)) parts.push(...data.friction_points.filter((s: any) => typeof s === 'string'));
    const text = parts.join(' ');
    if (!text) return null;

    // 1. github.com/<owner>/<repo> — canonical project usually equals repo name
    const ghMatches = [...text.matchAll(/github\.com\/[\w.-]+\/([\w.-]+)/gi)];
    for (const m of ghMatches) {
        const repo = m[1].replace(/\.git$/i, '');
        for (const p of validProjects) {
            if (p.toLowerCase() === repo.toLowerCase()) return p;
        }
    }

    // 2. Task ID prefix like "aops-e17e4e64", "brain-12ab", etc.
    for (const p of validProjects) {
        if (p.length < 2) continue;
        const escaped = p.replace(/[-/\\^$*+?.()|[\]{}]/g, '\\$&');
        const re = new RegExp(`\\b${escaped}-[0-9a-f]{4,}\\b`, 'i');
        if (re.test(text)) return p;
    }

    return null;
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

const PR_URL_RE = /https:\/\/github\.com\/[\w.-]+\/[\w.-]+\/pull\/\d+/;

// Scan timeline for any GitHub PR URL — used to infer DONE when /dump never ran.
function detectPrInTimeline(timeline: any[]): string | null {
    if (!Array.isArray(timeline)) return null;
    for (const evt of timeline) {
        const desc = evt?.description || '';
        const m = desc.match(PR_URL_RE);
        if (m) return m[0];
    }
    return null;
}

// Status thresholds
const RUNNING_WINDOW_MIN = 10;
const IDLE_TO_DRIFTED_MIN = 4 * 60;     // >4h silent w/ no outcome → drifted
const DONE_INFER_AFTER_PR_MIN = 60;     // PR filed + 1h quiet → infer done



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
 *
 * Returns:
 *   active     — sessions modified in the last `hours` hours, sorted newest-first.
 *   staleCount — count of sessions modified between `hours`h and `staleHoursMax`h ago.
 *                These are hidden from the main list and surfaced as an archive prompt
 *                (see task-4acd3722).
 */
async function findActiveSessions(
    hours = 24,
    staleHoursMax = 72,
): Promise<{ active: any[]; staleCount: number }> {
    if (!AOPS_SESSIONS) return { active: [], staleCount: 0 };
    const summariesDir = join(AOPS_SESSIONS, 'summaries');
    const cutoff = Date.now() - hours * 3600 * 1000;
    const staleCutoff = Date.now() - staleHoursMax * 3600 * 1000;
    const results: any[] = [];
    let staleCount = 0;

    let files: string[];
    try {
        files = await readdir(summariesDir);
    } catch {
        return { active: results, staleCount: 0 };
    }

    // Generate prefixes spanning the wider stale window so we can count >24h sessions too.
    const prefixDays = Math.max(4, Math.ceil(staleHoursMax / 24) + 1);
    const prefixes = Array.from({ length: prefixDays }, (_, i) => i).map(days => {
        const d = new Date(Date.now() - days * 86400000);
        const y = d.getFullYear();
        const m = String(d.getMonth() + 1).padStart(2, '0');
        const day = String(d.getDate()).padStart(2, '0');
        return `${y}${m}${day}`;
    });
    const candidates = files.filter(f => f.endsWith('.json') && prefixes.some(p => f.startsWith(p)));

    // First pass: read every recent summary, build the set of "real" project names
    // (anything non-empty, non-'unknown', not hash-like). Also count stale entries.
    type Entry = { filename: string; filePath: string; mtimeMs: number; data: any };
    const entries: Entry[] = [];
    const validProjects = new Set<string>();
    for (const filename of candidates) {
        const filePath = join(summariesDir, filename);
        let st;
        try {
            st = await stat(filePath);
        } catch {
            continue;
        }
        if (st.mtimeMs < staleCutoff) continue;
        if (st.mtimeMs < cutoff) {
            staleCount++;
            continue;
        }

        const data = await readJson(filePath);
        if (!data) continue;

        entries.push({ filename, filePath, mtimeMs: st.mtimeMs, data });
        const p = data.project;
        if (typeof p === 'string' && p && p !== 'unknown' && !isHashLikeProject(p)) {
            validProjects.add(p);
        }
    }

    // Second pass: classify each session, recovering or demoting hash-projected ones.
    for (const { filename, mtimeMs, data } of entries) {
        // Include all sessions within 24h; outcomes are now valid
        const rawProject: string = data.project || 'unknown';
        let project = rawProject;
        let projectOrphaned = false;
        if (isHashLikeProject(rawProject)) {
            const recovered = recoverProject(data, validProjects);
            if (recovered) {
                project = recovered;
            } else {
                project = 'unattributed';
                projectOrphaned = true;
            }
        }
        const minutesAgo = (Date.now() - mtimeMs) / 60000;
        const hoursAgo = minutesAgo / 60;

        // Classify session type from filename. Sessions whose project we couldn't recover
        // are demoted to 'orphan' so the dashboard buckets them as background activity
        // instead of treating them as full interactive sessions.
        const stem = filename.replace('.json', '');
        const isPolecat = stem.includes('polecat');
        const isCrew = stem.includes('crew');
        const isScheduled = stem.includes('scheduled');
        const isGha = stem.includes('gha');
        const sessionType: 'polecat' | 'crew' | 'scheduled' | 'gha' | 'orphan' | 'interactive' =
            isPolecat ? 'polecat'
            : isCrew ? 'crew'
            : isScheduled ? 'scheduled'
            : isGha ? 'gha'
            : projectOrphaned ? 'orphan'
            : 'interactive';

        // Extract the first meaningful user prompt from timeline as description
        // This is the best context for "what was this session about?"
        const timeline: any[] = data.timeline_events || [];
        // Use accomplishments/summary for completed sessions, otherwise fallback to clean prompt
        const cleanPrompt = extractCleanPrompt(timeline);
        let description = '';
        if (data.accomplishments && data.accomplishments.length > 0) {
            description = data.accomplishments[0];
        } else if (data.summary) {
            description = data.summary;
        } else if (cleanPrompt) {
            description = cleanPrompt.slice(0, 300);
        }
        
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

        // Status taxonomy: completed (terminal success — explicit /dump or inferred via PR),
        // abandoned (terminal failure outcome), running (<10m), idle (10m–4h, no outcome),
        // drifted (>4h silent, no outcome — probably ghost). See task-76f676a6.
        const prUrl: string | null = data.pr_url || detectPrInTimeline(timeline);
        let statusBadge: string;
        if (data.outcome === 'success') statusBadge = 'completed';
        else if (data.outcome) statusBadge = 'abandoned';
        else if (prUrl && minutesAgo > DONE_INFER_AFTER_PR_MIN) statusBadge = 'completed';
        else if (minutesAgo < RUNNING_WINDOW_MIN) statusBadge = 'running';
        else if (minutesAgo > IDLE_TO_DRIFTED_MIN) statusBadge = 'drifted';
        else statusBadge = 'idle';

        results.push({
            session_id: data.session_id || '',
            project,
            description,
            session_type: sessionType,
            // `surface` is the spec name (task-6f7f9f85) — same value as session_type, kept
            // dual-named while consumers migrate.
            surface: sessionType,
            started_at: data.date || new Date(mtimeMs).toISOString(),
            time_display: minutesAgo < 60 ? `${Math.round(minutesAgo)}m ago` : `${Math.round(hoursAgo)}h ago`,
            duration_min: durationMin,
            prompt_count: promptCount,
            id: data.session_id || stem,
            prompts: allPrompts,
            is_active: minutesAgo < 10,
            last_modified: mtimeMs,
            statusBadge: statusBadge,
            status_badge: statusBadge,
            needs_you: false,
            source: 'summaries',
            outcome: data.outcome || null,
            pr_url: prUrl,
            accomplishments: data.accomplishments || [],
            friction_points: data.friction_points || [],
            token_metrics: data.token_metrics || null,
            hostname: data.hostname || null,
            provider: data.provider || null,
            machine: data.machine || null,
            task_id: data.task_id || null
        });
    }

    results.sort((a, b) => b.last_modified - a.last_modified);
    return { active: results, staleCount };
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

    // Normalize hash-clobbered project fields the same way findActiveSessions does, so
    // recovered sessions feed into the right project's accomplishments rollup and orphans
    // don't pollute project lists.
    const validProjects = new Set<string>();
    for (const s of summaries) {
        const p = s.project;
        if (typeof p === 'string' && p && p !== 'unknown' && !isHashLikeProject(p)) {
            validProjects.add(p);
        }
    }
    for (const s of summaries) {
        if (isHashLikeProject(s.project)) {
            const recovered = recoverProject(s, validProjects);
            s.project = recovered ?? 'unattributed';
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
    const [sessionsResult, summaries, projectsConfig] = await Promise.all([
        findActiveSessions(24, 72), // 24h main window + count stale up to 72h
        loadRecentSummaries(3),
        loadProjectsConfig(),
    ]);

    const sessions = sessionsResult.active;
    const staleCount = sessionsResult.staleCount;

    // All sessions are active within the 24h window
    const activeSessions = sessions;
    const needsYouSessions = sessions.filter(s => s.needs_you);

    // Build project-level data: prefer graph data (via client), synthesis enriches
    // Collect all projects from sessions + synthesis
    const projectSet = new Set<string>();
    const projectLatestSession = new Map<string, number>();

    const pseudoProjects = new Set(projectsConfig.pseudo_projects);

    sessions.forEach(s => {
        if (s.project && s.project !== 'unattributed' && !pseudoProjects.has(s.project)) {
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
        // Accomplishments feed the project card's "Recently Completed" lane. Polecat
        // sessions go through the same pipeline (task-6f7f9f85), but we tag their
        // entries with surface='polecat' so the UI can badge them as autonomous output.
        for (const s of summaries) {
            if (s.project !== proj) continue;
            const filename: string = s._filename || '';
            const surface = filename.includes('polecat') ? 'polecat'
                : filename.includes('crew') ? 'crew'
                : filename.includes('scheduled') ? 'scheduled'
                : filename.includes('gha') ? 'gha'
                : 'interactive';
            const accs = s.accomplishments || [];
            if (accs.length > 0) {
                for (const text of accs) {
                    if (!seen.has(text)) {
                        seen.add(text);
                        projAccomplishments.push({ description: text, surface, session_id: s.session_id });
                    }
                }
            } else if (s.summary) {
                if (!seen.has(s.summary)) {
                    seen.add(s.summary);
                    projAccomplishments.push({ description: s.summary, surface, session_id: s.session_id });
                }
            }
        }
        projectData.accomplishments[proj] = projAccomplishments;
    }

    // Pipeline health — fail fast and loud when data sources are missing
    const summariesDirOk = AOPS_SESSIONS !== '';

    return {
        dashboardData: {
            pipeline_errors: [
                ...(!summariesDirOk ? ['$AOPS_SESSIONS not set — session discovery disabled'] : []),
            ],
            // Bucketed sessions for triage display
            active_agents: activeSessions,
            needs_you: needsYouSessions,
            stale_count: staleCount,
            synthesis: null,

            project_projects: projectProjects,
            project_data: projectData,
            path: buildPathData(summaries),
            projects_config: projectsConfig,
        },
    };
};
