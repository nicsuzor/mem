<script lang="ts">
    import { projectColor, projectBgTint, projectBorderColor } from "../../data/projectUtils";
    import { copyToClipboard } from "../../data/utils";
    import { toggleSelection, toggleSessionSelection } from "../../stores/selection";

    let { 
        sessions = [], 
        needsYou = [], 
        title = "CURRENT ACTIVITY",
        compact = false
    }: {
        sessions?: any[];
        needsYou?: any[];
        title?: string;
        compact?: boolean;
    } = $props();

    let isSubmitting = $state(false);
    let expandedSessions = $state<Record<string, boolean>>({});

    let groupedCompactSessions = $derived(
        sessions.reduce((acc: Record<string, any[]>, session: any) => {
            const p = session.project || 'unassigned';
            if (!acc[p]) acc[p] = [];
            acc[p].push(session);
            return acc;
        }, {})
    );

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



    function getTooltip(session: any): string {
        let tooltip = session.description || '';
        if (session.token_metrics) {
            const tm = session.token_metrics;
            const t = tm.totals || {};
            const e = tm.efficiency || {};
            const input = t.input_tokens ? Math.round(t.input_tokens/1000) + 'k' : '0';
            const output = t.output_tokens ? Math.round(t.output_tokens/1000) + 'k' : '0';
            const cacheHit = e.cache_hit_rate ? Math.round(e.cache_hit_rate * 100) + '%' : '0%';
            const tpm = e.tokens_per_minute ? Math.round(e.tokens_per_minute) : '0';
            
            tooltip += `\n\nTokens: In ${input} | Out ${output} | Cache: ${cacheHit} | Speed: ${tpm} tpm`;
        }
        return tooltip;
    }

    function getCompactTooltip(session: any): string {
        let tooltip = '';
        if (session.token_metrics) {
            const tm = session.token_metrics;
            const t = tm.totals || {};
            const e = tm.efficiency || {};
            const input = t.input_tokens ? Math.round(t.input_tokens/1000) + 'k' : '0';
            const output = t.output_tokens ? Math.round(t.output_tokens/1000) + 'k' : '0';
            const cacheHit = e.cache_hit_rate ? Math.round(e.cache_hit_rate * 100) + '%' : '0%';
            const tpm = e.tokens_per_minute ? Math.round(e.tokens_per_minute) : '0';
            
            tooltip += `Tokens: In ${input} | Out ${output} | Cache: ${cacheHit} | Speed: ${tpm} tpm\n\n`;
        }
        tooltip += `--- Transcripts ---\n`;
        if (session.prompts && session.prompts.length > 0) {
            tooltip += session.prompts.map((p: string, i: number) => `[${i+1}] ${p}`).join('\n\n');
        } else {
            tooltip += "No transcripts available.";
        }
        return tooltip;
    }

    const BADGE_STYLES: Record<string, { label: string; class: string }> = {
        running: { label: 'RUNNING', class: 'bg-primary text-black animate-pulse' },
        needs_you: { label: 'NEEDS YOU', class: 'bg-red-500 text-white animate-pulse' },
        errored: { label: 'ERRORED', class: 'bg-red-700 text-white' },
        completed: { label: 'DONE', class: 'bg-green-900/40 text-green-400 border border-green-500/30' },
        abandoned: { label: 'ABANDONED', class: 'bg-primary/10 text-primary/40 border border-primary/20 line-through' },
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

    <!-- Active Sessions -->
    {#if compact}
        <div class="flex flex-col gap-4 mt-2">
            {#each Object.entries(groupedCompactSessions).sort((a, b) => b[1].length - a[1].length) as [project, projSessions]}
                <div class="flex flex-col gap-1.5">
                    <div class="text-[10px] font-bold uppercase tracking-widest flex items-center gap-2" style="color: {project !== 'unassigned' ? projectColor(project) : '#888'};">
                        <div class="w-1.5 h-1.5 rounded-full" style="background: {project !== 'unassigned' ? projectColor(project) : '#888'};"></div>
                        {project} ({projSessions.length})
                    </div>
                    <div class="flex flex-wrap gap-1">
                        {#each projSessions as session}
                            {@const shortId = (session.session_id || "").slice(-8)}
                            <div class="flex items-center gap-1 bg-primary/5 border border-primary/10 px-1.5 py-0.5 hover:bg-primary/20 transition-colors cursor-help group"
                                 title={getCompactTooltip(session)}>
                                <button class="text-[10px] font-mono text-primary/70 group-hover:text-white transition-colors" 
                                        onclick={(e) => { e.stopPropagation(); copyToClipboard(session.session_id); }}
                                        title="Click to copy session ID: {session.session_id}">
                                    {shortId}
                                </button>
                                <button class="text-primary/30 hover:text-primary transition-colors flex items-center opacity-0 group-hover:opacity-100" 
                                        onclick={(e) => { e.stopPropagation(); toggleSessionSelection(session.session_id); }}
                                        title="View detailed session metadata">
                                    <span class="material-symbols-outlined text-[12px] leading-none">info</span>
                                </button>
                            </div>
                        {/each}
                    </div>
                </div>
            {/each}
            {#if sessions.length === 0}
                <div class="flex items-center gap-3 text-xs text-primary/50 py-2">
                    <span class="material-symbols-outlined text-[16px] text-primary/30">nights_stay</span>
                    No background activity.
                </div>
            {/if}
        </div>
    {:else}
        <div class="flex flex-col gap-2">
            {#each sessions as session}
                {@const expanded = !!expandedSessions[session.session_id]}
                {@const timeline = session.prompts || []}
                {@const shortId = (session.session_id || "").slice(-8)}
                <div class="bg-primary/5 border-l-2 {session.needs_you ? 'border-red-500' : 'border-primary/50'} hover:bg-primary/10 transition-colors" title={getTooltip(session)}>
                    <div class="flex items-center gap-4 p-2 cursor-pointer"
                         role="button" tabindex="0" onclick={() => toggleExpand(session.session_id)} onkeydown={(e) => { if(e.key === 'Enter') toggleExpand(session.session_id); }}>
                        <span class="text-[10px] text-primary/60 min-w-[55px]">{formatTimeAgo(session.started_at)}</span>
                        
                        {#if session.session_id}
                            <div class="flex items-center gap-1 shrink-0">
                                <button class="text-[9px] font-bold bg-primary/20 text-primary/60 px-1 py-0.5 hover:bg-primary/40 transition-colors" 
                                        onclick={(e) => { e.stopPropagation(); copyToClipboard(session.session_id); }}
                                        title="Click to copy session ID: {session.session_id}">
                                    {shortId}
                                </button>
                                <button class="text-[9px] font-bold bg-primary/20 text-primary/60 px-1 py-0.5 hover:bg-primary/40 transition-colors" 
                                        onclick={(e) => { e.stopPropagation(); toggleSessionSelection(session.session_id); }}
                                        title="View detailed session metadata">
                                    <span class="material-symbols-outlined text-[12px] leading-none">info</span>
                                </button>
                            </div>
                        {/if}

                        {#if session.project}
                            <span class="text-[10px] font-bold px-2 py-0.5"
                                  style="background: {projectBgTint(session.project)}; color: {projectColor(session.project)}; border: 1px solid {projectBorderColor(session.project)};">{session.project}</span>
                        {/if}
                        <span class="text-xs text-primary/90 flex-1 {expanded ? 'whitespace-pre-wrap break-words' : 'truncate'}">
                            {session.description}
                        </span>
                        {#if session.prompt_count != null}
                            <span class="text-[10px] text-primary/40 shrink-0" title="User prompts">{session.prompt_count}p</span>
                        {/if}
                        {#if session.status_badge}
                            {@const badge = BADGE_STYLES[session.status_badge] || BADGE_STYLES.idle}
                            <span class="text-[10px] font-bold px-1.5 py-0.5 {badge.class} shrink-0">{badge.label}</span>
                        {/if}
                    </div>
                    {#if expanded}
                        <div class="flex flex-col gap-3 px-4 py-3 border-t border-primary/10 bg-black/20">
                            {#if session.accomplishments?.length > 0}
                                <div class="flex flex-col gap-1">
                                    <span class="text-[10px] font-bold text-green-500 tracking-widest uppercase">Accomplishments</span>
                                    {#each session.accomplishments as acc}
                                        <div class="text-[11px] text-primary/80 flex gap-2"><span class="text-green-500">›</span> {acc}</div>
                                    {/each}
                                </div>
                            {/if}
                            {#if session.friction_points?.length > 0}
                                <div class="flex flex-col gap-1">
                                    <span class="text-[10px] font-bold text-yellow-500 tracking-widest uppercase">Friction Points</span>
                                    {#each session.friction_points as fp}
                                        <div class="text-[11px] text-primary/80 flex gap-2"><span class="text-yellow-500">!</span> {fp}</div>
                                    {/each}
                                </div>
                            {/if}
                            {#if session.outcome}
                                <div class="flex flex-col gap-1">
                                    <span class="text-[10px] font-bold text-primary/50 tracking-widest uppercase">Outcome</span>
                                    <div class="text-[11px] text-primary/60">{session.outcome}</div>
                                </div>
                            {/if}
                            {#if timeline.length > 0}
                                <div class="flex flex-col gap-1 mt-1">
                                    <span class="text-[10px] font-bold text-primary/50 tracking-widest uppercase">Timeline</span>
                                    {#each timeline as prompt}
                                        <p class="text-[11px] text-primary/60 py-1 border-b border-primary/5 last:border-0 whitespace-pre-wrap break-words">›_ {prompt}</p>
                                    {/each}
                                </div>
                            {/if}
                        </div>
                    {/if}
                </div>
            {/each}
            {#if sessions.length === 0}
                <div class="flex items-center gap-3 text-xs text-primary/50 py-2">
                    <span class="material-symbols-outlined text-[16px] text-primary/30">nights_stay</span>
                    All quiet — no active sessions right now.
                </div>
            {/if}
        </div>
    {/if}
</div>
