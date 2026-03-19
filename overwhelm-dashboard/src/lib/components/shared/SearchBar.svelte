<script lang="ts">
    import { selection } from '../../stores/selection';

    interface SearchResult {
        id: string;
        title: string;
        status?: string;
        priority?: number;
        score: number;
    }

    let query = '';
    let results: SearchResult[] = [];
    let loading = false;
    let error = '';
    let debounceTimer: ReturnType<typeof setTimeout> | null = null;
    let open = false;

    function onInput() {
        if (debounceTimer) clearTimeout(debounceTimer);
        error = '';
        if (!query.trim()) {
            results = [];
            open = false;
            return;
        }
        debounceTimer = setTimeout(doSearch, 300);
    }

    async function doSearch() {
        loading = true;
        try {
            const res = await fetch(`/api/search?q=${encodeURIComponent(query)}&limit=10`);
            const data = await res.json();
            if (res.ok) {
                results = data.results ?? [];
                open = results.length > 0;
            } else {
                error = data.error ?? 'Search failed';
                results = [];
                open = false;
            }
        } catch (e: any) {
            error = 'Search unavailable';
            results = [];
            open = false;
        } finally {
            loading = false;
        }
    }

    function selectResult(id: string) {
        selection.update(s => ({ ...s, activeNodeId: id }));
        open = false;
        query = '';
        results = [];
    }

    function onKeydown(e: KeyboardEvent) {
        if (e.key === 'Escape') {
            open = false;
            query = '';
            results = [];
        }
    }

    const STATUS_COLORS: Record<string, string> = {
        done: 'text-green-500/60',
        active: 'text-primary',
        blocked: 'text-red-400',
        ready: 'text-yellow-400',
    };
</script>

<svelte:window onkeydown={onKeydown} />

<div class="relative w-full">
    <div class="relative flex items-center">
        <span class="absolute left-2 material-symbols-outlined text-[14px] text-primary/40 pointer-events-none">
            {loading ? 'sync' : 'search'}
        </span>
        <input
            type="text"
            bind:value={query}
            oninput={onInput}
            placeholder="Search PKB..."
            class="w-full bg-black/50 border border-primary/30 text-primary text-xs pl-7 pr-3 py-1.5 focus:ring-1 focus:ring-primary outline-none font-mono placeholder:text-primary/30 {loading ? 'animate-pulse' : ''}"
        />
    </div>

    {#if error}
        <p class="text-[9px] text-destructive mt-1 font-mono">{error}</p>
    {/if}

    {#if open && results.length > 0}
        <div class="absolute top-full left-0 right-0 z-50 mt-1 bg-background border border-primary/40 shadow-xl max-h-72 overflow-y-auto custom-scrollbar">
            {#each results as result}
                <button
                    class="w-full text-left px-3 py-2 hover:bg-primary/10 border-b border-primary/10 last:border-0 transition-colors"
                    onclick={() => selectResult(result.id)}
                >
                    <div class="flex items-center justify-between gap-2">
                        <span class="text-[11px] text-primary font-mono truncate">{result.title}</span>
                        {#if result.status}
                            <span class="text-[9px] font-bold uppercase shrink-0 {STATUS_COLORS[result.status] ?? 'text-primary/50'}">
                                {result.status}
                            </span>
                        {/if}
                    </div>
                    <div class="flex items-center gap-2 mt-0.5">
                        <span class="text-[9px] text-primary/40 font-mono">{result.id}</span>
                        {#if result.priority !== undefined}
                            <span class="text-[9px] text-primary/40">P{result.priority}</span>
                        {/if}
                    </div>
                </button>
            {/each}
        </div>
    {/if}
</div>
