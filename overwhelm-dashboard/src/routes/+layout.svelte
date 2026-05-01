<script lang="ts">
	import favicon from "$lib/assets/favicon.svg";
	import "../app.css";
	import { viewSettings, VIEW_MODES } from "$lib/stores/viewSettings";
	import QuickCapture from "$lib/components/dashboard/QuickCapture.svelte";
	import Toast from "$lib/components/shared/Toast.svelte";

	let { children } = $props();
	let mobileMenuOpen = $state(false);

	function openTab(tab: 'Dashboard' | 'Task Graph' | 'Threaded Tasks' | 'Insights', mode?: typeof VIEW_MODES[number]) {
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
			<div class="size-6 text-primary animate-pulse" aria-hidden="true">
				<span class="material-symbols-outlined" style="font-size: 24px;">terminal</span>
			</div>
			<h1 class="text-lg font-bold tracking-widest text-primary text-glow uppercase">aOps Dashboard</h1>
		</div>

		<!-- Navigation Links -->
		<nav class="hidden lg:flex items-center gap-8" aria-label="Primary navigation">
			<button
				class="text-xs font-bold uppercase transition-colors border-b-2 {$viewSettings.mainTab === 'Dashboard' ? 'border-primary text-primary' : 'border-transparent text-primary/60 hover:text-primary hover:border-primary/50'}"
				onclick={() => openTab('Dashboard')}
				aria-label="Go to Dashboard"
				aria-current={$viewSettings.mainTab === 'Dashboard' ? 'page' : undefined}
			>
				DASHBOARD
			</button>

			<button
				class="text-xs font-bold uppercase transition-colors border-b-2 {$viewSettings.mainTab === 'Task Graph' ? 'border-primary text-primary' : 'border-transparent text-primary/60 hover:text-primary hover:border-primary/50'}"
				onclick={() => openTab('Task Graph')}
				aria-label="Go to Task Graph"
				aria-current={$viewSettings.mainTab === 'Task Graph' ? 'page' : undefined}
			>
				GRAPH
			</button>

			<button
				class="text-xs font-bold uppercase transition-colors border-b-2 {$viewSettings.mainTab === 'Threaded Tasks' ? 'border-primary text-primary' : 'border-transparent text-primary/60 hover:text-primary hover:border-primary/50'}"
				onclick={() => openTab('Threaded Tasks')}
				aria-label="Go to Threaded Tasks"
				aria-current={$viewSettings.mainTab === 'Threaded Tasks' ? 'page' : undefined}
			>
				TASKS
			</button>

			<button
				class="text-xs font-bold uppercase transition-colors border-b-2 {$viewSettings.mainTab === 'Insights' ? 'border-primary text-primary' : 'border-transparent text-primary/60 hover:text-primary hover:border-primary/50'}"
				onclick={() => openTab('Insights')}
				aria-label="Go to Insights"
				aria-current={$viewSettings.mainTab === 'Insights' ? 'page' : undefined}
			>
				INSIGHTS
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
			<nav class="flex max-h-[calc(100vh-3.5rem)] flex-col gap-2 overflow-y-auto px-4 py-4" aria-label="Mobile primary navigation">
				<button
					class="mobile-nav-link {$viewSettings.mainTab === 'Dashboard' ? 'mobile-nav-link-active' : ''}"
					onclick={() => openTab('Dashboard')}
					aria-label="Go to Dashboard"
					aria-current={$viewSettings.mainTab === 'Dashboard' ? 'page' : undefined}
				>
					Dashboard
				</button>

				<button
					class="mobile-nav-link {$viewSettings.mainTab === 'Task Graph' ? 'mobile-nav-link-active' : ''}"
					onclick={() => openTab('Task Graph')}
					aria-label="Go to Task Graph"
					aria-current={$viewSettings.mainTab === 'Task Graph' ? 'page' : undefined}
				>
					Graph
				</button>
				
				{#if $viewSettings.mainTab === 'Task Graph'}
					<div class="grid grid-cols-2 gap-2 pl-4 py-2 border-l border-primary/20">
						{#each VIEW_MODES as mode}
							<button
								class="text-left text-[10px] font-bold uppercase tracking-widest p-2 transition-colors {$viewSettings.viewMode === mode ? 'text-primary bg-primary/10' : 'text-primary/50 hover:text-primary/80'}"
								onclick={() => openTab('Task Graph', mode)}
							>
								{mode}
							</button>
						{/each}
					</div>
				{/if}

				<button
					class="mobile-nav-link {$viewSettings.mainTab === 'Threaded Tasks' ? 'mobile-nav-link-active' : ''}"
					onclick={() => openTab('Threaded Tasks')}
					aria-label="Go to Threaded Tasks"
					aria-current={$viewSettings.mainTab === 'Threaded Tasks' ? 'page' : undefined}
				>
					Tasks
				</button>

				<button
					class="mobile-nav-link {$viewSettings.mainTab === 'Insights' ? 'mobile-nav-link-active' : ''}"
					onclick={() => openTab('Insights')}
					aria-label="Go to Insights"
					aria-current={$viewSettings.mainTab === 'Insights' ? 'page' : undefined}
				>
					Insights
				</button>
			</nav>
		</div>
	{/if}
</header>

<!-- View Mode Bar (Sub-navigation for Graph) -->
{#if $viewSettings.mainTab === 'Task Graph'}
	<div class="bg-surface/50 border-b border-primary/10 px-4 py-1 flex items-center gap-1 overflow-x-auto no-scrollbar relative z-20 h-10">
		<span class="text-[9px] font-black text-primary/30 uppercase tracking-[0.2em] mr-2">Views</span>
		{#each VIEW_MODES as mode}
			<button
				class="px-3 py-1.5 text-[10px] font-bold uppercase tracking-widest transition-all rounded {
					$viewSettings.viewMode === mode 
					? 'bg-primary/15 text-primary border border-primary/30' 
					: 'text-primary/50 hover:text-primary/80 hover:bg-primary/5 border border-transparent'
				}"
				onclick={() => $viewSettings.viewMode = mode}
			>
				{mode}
			</button>
		{/each}
	</div>
{/if}

<!-- Main Content Area Wrapper -->
<main class="flex-1 grid grid-cols-12 gap-0 overflow-hidden relative z-0 {$viewSettings.mainTab === 'Task Graph' ? 'h-[calc(100vh-6rem)]' : 'h-[calc(100vh-3.5rem)]'}">
	{@render children()}
</main>

<!-- Quick Capture available from any view (Alt+C) -->
<QuickCapture />
