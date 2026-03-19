<script lang="ts">
    let captureText = "";
    let isSubmitting = false;
    let lastCreatedId: string | null = null;
    let errorMsg: string | null = null;

    async function handleCapture() {
        if (!captureText.trim()) return;
        isSubmitting = true;
        lastCreatedId = null;
        errorMsg = null;

        try {
            const res = await fetch('/api/tasks/create', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ title: captureText.trim(), priority: 2 }),
            });
            const data = await res.json();
            if (res.ok && data.ok) {
                lastCreatedId = data.id;
                captureText = "";
            } else {
                errorMsg = data.error ?? 'Failed to capture';
            }
        } catch (e: any) {
            errorMsg = e.message ?? 'Network error';
        } finally {
            isSubmitting = false;
        }
    }
</script>

<div class="flex flex-col gap-4 font-mono">
    <h3 class="text-xs font-bold tracking-[0.2em] text-primary/80 border-b border-primary/30 pb-2 flex items-center gap-2">
        <span class="material-symbols-outlined text-[16px]">edit_note</span>
        QUICK CAPTURE
    </h3>

    <form on:submit|preventDefault={handleCapture} class="flex flex-col gap-3">
        <textarea
            bind:value={captureText}
            placeholder="Type a thought, task, or realization... (Alt+C)"
            disabled={isSubmitting}
            class="w-full h-24 bg-black/50 border border-primary/30 p-3 text-xs text-primary focus:border-primary focus:ring-1 focus:ring-primary outline-none transition-all resize-none placeholder:text-primary/30"
        ></textarea>

        <button
            class="w-full bg-primary/10 border border-primary text-primary text-xs font-bold tracking-widest py-2 hover:bg-primary hover:text-black transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            type="submit"
            disabled={isSubmitting || !captureText.trim()}
        >
            {#if isSubmitting}
                <span class="animate-pulse">CAPTURING...</span>
            {:else}
                CAPTURE NOTE
            {/if}
        </button>
    </form>

    {#if lastCreatedId}
        <p class="text-[10px] font-mono text-primary/60">
            CAPTURED: <span class="text-primary">{lastCreatedId}</span>
        </p>
    {/if}
    {#if errorMsg}
        <p class="text-[10px] font-mono text-destructive">{errorMsg}</p>
    {/if}
</div>
