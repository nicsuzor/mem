<script lang="ts">
	import favicon from "$lib/assets/favicon.svg";
	import "../app.css";
	import { viewSettings, VIEW_MODES } from "$lib/stores/viewSettings";
	import QuickCapture from "$lib/components/dashboard/QuickCapture.svelte";
	import Toast from "$lib/components/shared/Toast.svelte";

	let { children } = $props();
	let mobileMenuOpen = $state(false);

	function openTab(tab: 'Dashboard' | 'Task Graph' | 'Threaded Tasks', mode?: typeof VIEW_MODES[number]) {
		$viewSettings.mainTab = tab;
		if (mode) {
			$viewSettings.viewMode = mode;
		}
		mobileMenuOpen = false;
	}
</script>

<Toast />
<svelte:head>
	<link rel="icon" href={favicon} />
</svelte:head>

<!-- Scanline Overlay -->
<div class="scanlines"></div>

<!-- Header -->
<header class="flex-none h-14 border-b border-primary/30 bg-surface px-4 flex items-center justify-between z-10 relative">
	<div class="flex items-center gap-8">
		<div class="flex items-center gap-4">
			<div class="size-6 text-primary animate-pulse">
				<span class="material-symbols-outlined" style="font-size: 24px;">terminal</span>
			</div>
			<h1 class="text-lg font-bold tracking-widest text-primary text-glow">OPERATOR SYSTEM <span class="text-xs opacity-60 align-top">v1.0</span></h1>
		</div>

		<!-- Navigation Links -->
		<nav class="hidden lg:flex items-center gap-6">
			<button
				class="text-xs font-bold uppercase transition-colors border-b-2 {$viewSettings.mainTab === 'Dashboard' ? 'border-primary text-primary' : 'border-transparent text-primary/60 hover:text-primary hover:border-primary/50'}"
				onclick={() => openTab('Dashboard')}
			>
				DASHBOARD
			</button>

			<span class="text-primary/20">|</span>

			{#each VIEW_MODES as mode}
				<button
					class="text-xs font-bold uppercase transition-colors border-b-2 {$viewSettings.mainTab === 'Task Graph' && $viewSettings.viewMode === mode ? 'border-primary text-primary' : 'border-transparent text-primary/60 hover:text-primary hover:border-primary/50'}"
					onclick={() => openTab('Task Graph', mode)}
				>
					{mode}
				</button>
			{/each}

			<span class="text-primary/20">|</span>

			<button
				class="text-xs font-bold uppercase transition-colors border-b-2 {$viewSettings.mainTab === 'Threaded Tasks' ? 'border-primary text-primary' : 'border-transparent text-primary/60 hover:text-primary hover:border-primary/50'}"
				onclick={() => openTab('Threaded Tasks')}
			>
				THREADED TASKS
			</button>
		</nav>
	</div>

	<div class="flex items-center gap-3">
		<button
			class="lg:hidden inline-flex items-center gap-2 px-3 py-2 border border-primary/30 bg-primary/8 text-primary text-[11px] font-bold uppercase tracking-[0.18em] hover:bg-primary/12 transition-colors"
			type="button"
			aria-expanded={mobileMenuOpen}
			aria-controls="mobile-nav"
			onclick={() => mobileMenuOpen = !mobileMenuOpen}
		>
			<span class="material-symbols-outlined" style="font-size: 18px;">{mobileMenuOpen ? 'close' : 'menu'}</span>
			<span>Menu</span>
		</button>
	</div>

	{#if mobileMenuOpen}
		<div id="mobile-nav" class="absolute top-full left-0 right-0 lg:hidden border-b border-primary/30 bg-surface/96 backdrop-blur-md shadow-[0_16px_40px_rgba(0,0,0,0.45)]">
			<nav class="flex max-h-[calc(100vh-3.5rem)] flex-col gap-2 overflow-y-auto px-4 py-4">
				<button
					class="mobile-nav-link {$viewSettings.mainTab === 'Dashboard' ? 'mobile-nav-link-active' : ''}"
					onclick={() => openTab('Dashboard')}
				>
					Dashboard
				</button>

				<div class="px-1 pt-3 text-[10px] font-bold uppercase tracking-[0.24em] text-primary/45">Task Graph</div>
				{#each VIEW_MODES as mode}
					<button
						class="mobile-nav-link {$viewSettings.mainTab === 'Task Graph' && $viewSettings.viewMode === mode ? 'mobile-nav-link-active' : ''}"
						onclick={() => openTab('Task Graph', mode)}
					>
						{mode}
					</button>
				{/each}

				<div class="mt-2 border-t border-primary/10 pt-2">
					<button
						class="mobile-nav-link {$viewSettings.mainTab === 'Threaded Tasks' ? 'mobile-nav-link-active' : ''}"
						onclick={() => openTab('Threaded Tasks')}
					>
						Threaded Tasks
					</button>
				</div>
			</nav>
		</div>
	{/if}
</header>

<!-- Main Content Area Wrapper -->
<main class="flex-1 grid grid-cols-12 gap-0 overflow-hidden relative z-0 h-[calc(100vh-3.5rem)]">
	{@render children()}
</main>

<!-- Quick Capture available from any view (Alt+C) -->
<QuickCapture />
