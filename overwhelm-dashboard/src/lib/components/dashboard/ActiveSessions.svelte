<script lang="ts">
    import { projectColor, projectBgTint, projectBorderColor } from "../../data/projectUtils";
    import { copyToClipboard } from "../../data/utils";
    import { toggleSelection } from "../../stores/selection";

    let { 
        sessions = [], 
        pausedSessions = [], 
        staleSessions = [], 
        needsYou = [], 
        title = "CURRENT ACTIVITY",
        compact = false
    }: {
        sessions?: any[];
        pausedSessions?: any[];
        staleSessions?: any[];
        needsYou?: any[];
        title?: string;
        compact?: boolean;
    } = $props();

    let showPaused = $state(false);
    let isSubmitting = $state(false);
    let expandedSessions = $state<Record<string, boolean>>({});

    function toggleExpand(sessionId: string) {
        if (compact) return;
        expandedSessions = { ...expandedSessions, [sessionId]: !expandedSessions[sessionId] };
    }

    function formatTimeAgo(isoString: string): string {
        if (!isoString) return "just started";
        const date = new Date(isoString);
        const diffMs = Date.now() - date.getTime();
        const diffMins = Math.floor(diffMs / 60000);

        if (diffMins < 60) return `${diffMins}m ago`;
        const diffHrs = Math.floor(diffMins / 60);
        return `${diffHrs}h ago`;
    }

    async function dismissStaleSession(session: any) {
        if (session.source !== 'pkb' || !session.id || isSubmitting) return;
        isSubmitting = true;
        try {
            await fetch('/api/task/status', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ id: session.id, status: 'active' }), // Demote from in_progress
            });
            // Let the graph sync interval handle the UI update
        } catch (e) {
            console.error('Failed to dismiss session', e);
        } finally {
            isSubmitting = false;
        }
    }

    async function dismissAllStale() {
        if (isSubmitting) return;
        isSubmitting = true;
        try {
            // Dismiss all pkb-sourced stale sessions
            const pkbStale = staleSessions.filter(s => s.source === 'pkb' && s.id);
            await Promise.all(pkbStale.map(s => 
                fetch('/api/task/status', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ id: s.id, status: 'active' }),
                })
            ));
        } catch (e) {
            console.error('Failed to dismiss all sessions', e);
        } finally {
            isSubmitting = false;
        }
    }

    const BADGE_STYLES: Record<string, { label: string; class: string }> = {
        running: { label: 'RUNNING', class: 'bg-primary text-black animate-pulse' },
        needs_you: { label: 'NEEDS YOU', class: 'bg-red-500 text-white animate-pulse' },
        errored: { label: 'ERRORED', class: 'bg-red-700 text-white' },
        completed: { label: 'DONE', class: 'bg-green-700 text-white' },
        paused: { label: 'PAUSED', class: 'bg-primary/30 text-primary/70' },
        idle: { label: 'IDLE', class: 'bg-primary/20 text-primary/50' },
    };
</script>

<div class="flex flex-col gap-4 font-mono w-full">
    <div class="flex justify-between items-center border-b border-primary/30 pb-2">
        <h3 class="text-sm font-bold tracking-widest text-primary flex items-center gap-2">
            <span class="material-symbols-outlined text-[16px]">{compact ? 'robot_2' : 'bolt'}</span>
            {title} ({sessions.length})
        </h3>
        {#if needsYou.length > 0 && !compact}
            <div class="flex items-center gap-2 px-3 py-1 border border-red-500 bg-red-900/20 text-red-500 font-bold text-[10px] uppercase tracking-widest animate-pulse">
                <span class="material-symbols-outlined text-[14px]">warning</span>
                {needsYou.length} Needs You
            </div>
        {/if}
    </div>

    <!-- Active Sessions (< 4h) — full cards -->
    <div class="flex flex-col gap-2">
        {#each (compact ? sessions.slice(0, 15) : sessions.slice(0, 8)) as session}
            {@const expanded = !!expandedSessions[session.session_id]}
            {@const extraPrompts = (session.prompts || []).slice(1)}
            {@const shortId = (session.session_id || "").slice(-8)}
            <div class="bg-primary/5 border-l-2 {session.needs_you ? 'border-red-500' : 'border-primary/50'} hover:bg-primary/10 transition-colors">
                <div class="flex items-center gap-4 p-2 cursor-pointer"
                     role="button" tabindex="0" onclick={() => toggleExpand(session.session_id)} onkeydown={(e) => { if(e.key === 'Enter') toggleExpand(session.session_id); }}>
                    <span class="text-[10px] text-primary/60 min-w-[55px]">{formatTimeAgo(session.started_at)}</span>
                    
                    {#if !compact && session.session_id}
                        <button class="text-[9px] font-bold bg-primary/20 text-primary/60 px-1 py-0.5 hover:bg-primary/40 transition-colors shrink-0" 
                                onclick={(e) => { e.stopPropagation(); copyToClipboard(session.session_id); }}
                                title="Click to copy session ID: {session.session_id}">
                            {shortId}
                        </button>
                    {/if}

                    {#if session.project}
                        <span class="text-[10px] font-bold px-2 py-0.5"
                              style="background: {projectBgTint(session.project)}; color: {projectColor(session.project)}; border: 1px solid {projectBorderColor(session.project)};">{session.project}</span>
                    {/if}
                    <span class="text-xs text-primary/90 flex-1 {expanded ? 'whitespace-pre-wrap break-words' : 'truncate'}" title={session.description}>
                        {session.description}
                    </span>
                    {#if session.prompt_count != null && !compact}
                        <span class="text-[10px] text-primary/40 shrink-0" title="User prompts">{session.prompt_count}p</span>
                    {/if}
                    {#if session.status_badge && !compact}
                        {@const badge = BADGE_STYLES[session.status_badge] || BADGE_STYLES.idle}
                        <span class="text-[10px] font-bold px-1.5 py-0.5 {badge.class} shrink-0">{badge.label}</span>
                    {/if}
                </div>
                {#if expanded && extraPrompts.length > 0}
                    <div class="flex flex-col gap-1 px-4 pb-2 border-t border-primary/10">
                        {#each extraPrompts as prompt}
                            <p class="text-[11px] text-primary/60 py-1 border-b border-primary/5 last:border-0 whitespace-pre-wrap break-words">{prompt}</p>
                        {/each}
                    </div>
                {/if}
            </div>
        {/each}
        {#if sessions.length === 0}
            <div class="flex items-center gap-3 text-xs text-primary/50 py-2">
                <span class="material-symbols-outlined text-[16px] text-primary/30">nights_stay</span>
                {compact ? 'No background activity.' : 'All quiet — no active sessions right now.'}
            </div>
        {/if}
    </div>

    <!-- Paused Sessions (4-24h) — collapsed, expandable -->
    {#if pausedSessions.length > 0}
        <button
            class="flex items-center gap-2 text-[10px] font-bold tracking-widest text-primary/50 hover:text-primary transition-colors cursor-pointer border-t border-primary/10 pt-3"
            onclick={() => showPaused = !showPaused}
        >
            <span class="material-symbols-outlined text-[14px]">{showPaused ? 'expand_less' : 'expand_more'}</span>
            PAUSED ({pausedSessions.length}) — 4-24h ago
        </button>
        {#if showPaused}
            <div class="flex flex-col gap-1 opacity-60">
                {#each pausedSessions.slice(0, 10) as session}
                    <div class="flex items-center gap-4 bg-primary/3 border-l border-primary/20 p-1.5 text-xs cursor-pointer hover:bg-primary/10"
                         role="button" tabindex="0" onclick={() => { if(session.id) toggleSelection(session.id); }} onkeydown={(e) => { if(e.key === 'Enter' && session.id) toggleSelection(session.id); }}>
                        <span class="text-[10px] text-primary/40 min-w-[55px]">{session.time_display}</span>
                        {#if session.project}
                            <span class="text-[10px] font-bold px-1.5 py-0.5"
                                  style="background: {projectBgTint(session.project)}; color: {projectColor(session.project)}; border: 1px solid {projectBorderColor(session.project)};">{session.project}</span>
                        {/if}
                        <span class="text-primary/60 truncate flex-1" title={session.description}>{session.description}</span>
                        {#if session.status_badge}
                            {@const badge = BADGE_STYLES[session.status_badge] || BADGE_STYLES.paused}
                            <span class="text-[9px] font-bold px-1 py-0.5 {badge.class} shrink-0">{badge.label}</span>
                        {/if}
                    </div>
                {/each}
                {#if pausedSessions.length > 10}
                    <div class="text-[10px] text-primary/30 italic pl-2">+ {pausedSessions.length - 10} more paused</div>
                {/if}
            </div>
        {/if}
    {/if}

    <!-- Stale Sessions (>24h) — archive prompt per spec -->
    {#if staleSessions.length > 0}
        <div class="flex flex-col gap-2 mt-2">
            {#each staleSessions.slice(0, 5) as session}
                <div class="flex items-center gap-3 border border-primary/20 bg-primary/5 p-3">
                    <span class="material-symbols-outlined text-[16px] text-primary/40">inventory_2</span>
                    <div class="flex flex-col gap-1 flex-1 min-w-0">
                        <div class="flex items-center gap-2">
                            {#if session.project}
                                <span class="text-[9px] font-bold px-1 py-0.5"
                                      style="background: {projectBgTint(session.project)}; color: {projectColor(session.project)}; border: 1px solid {projectBorderColor(session.project)};">{session.project}</span>
                            {/if}
                            <span class="text-[10px] text-primary/50">{session.time_display}</span>
                        </div>
                        <span class="text-xs text-primary/70 truncate" title={session.description}>{session.description}</span>
                    </div>
                    <div class="flex items-center gap-2 shrink-0">
                        {#if session.id}
                            <button class="text-[10px] font-bold tracking-widest text-primary/50 hover:text-primary border border-primary/20 hover:border-primary/50 px-2 py-1 transition-colors"
                                    onclick={() => toggleSelection(session.id)}>
                                REVIEW
                            </button>
                        {/if}
                        {#if session.source === 'pkb'}
                            <button class="text-[10px] font-bold tracking-widest text-primary/30 hover:text-primary/60 px-2 py-1 transition-colors disabled:opacity-50"
                                    disabled={isSubmitting}
                                    onclick={() => dismissStaleSession(session)}>
                                DISMISS
                            </button>
                        {/if}
                    </div>
                </div>
            {/each}
            {#if staleSessions.length > 5}
                <div class="text-xs text-primary/50 pl-2">+ {staleSessions.length - 5} more stale sessions</div>
            {/if}
        </div>
    {/if}
</div>
