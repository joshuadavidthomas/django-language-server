<script lang="ts">
	import { Badge } from '#lib/components/ui/badge/index.js';
	import { ScrollArea } from '#lib/components/ui/scroll-area/index.js';
	import { getPlayground } from '#lib/playground.svelte.js';
	import CircleX from '@lucide/svelte/icons/circle-x';
	import Info from '@lucide/svelte/icons/info';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';

	const playground = getPlayground();
</script>

<section class="flex h-full min-h-0 flex-col" aria-labelledby="problems-title">
	<header class="flex h-9 shrink-0 items-center gap-2 border-b px-4">
		<h2 id="problems-title" class="text-sm/6 font-semibold">Problems</h2>
		<Badge variant="secondary" class="tabular-nums">{playground.problems.length}</Badge>
	</header>
	<ScrollArea class="min-h-0 flex-1">
		{#if playground.problems.length}
			<ul class="py-1">
				{#each playground.problems as problem, i (i)}
					{@const { line, character } = problem.diagnostic.range.start}
					{@const severity = problem.diagnostic.severity ?? 1}
					<li>
						<button
							type="button"
							class="flex w-full items-center gap-2 px-4 py-1 text-left text-sm/6 hover:bg-muted"
							onclick={() => playground.reveal(problem)}
						>
							{#if severity === 1}
								<CircleX class="size-4 shrink-0 text-destructive" aria-label="Error" />
							{:else if severity === 2}
								<TriangleAlert
									class="size-4 shrink-0 text-amber-600 dark:text-amber-400"
									aria-label="Warning"
								/>
							{:else}
								<Info class="size-4 shrink-0 text-muted-foreground" aria-label="Info" />
							{/if}
							<span class="truncate">{problem.diagnostic.message}</span>
							{#if problem.diagnostic.code !== undefined}
								<span class="shrink-0 font-mono text-xs text-muted-foreground">
									{problem.diagnostic.code}
								</span>
							{/if}
							<span class="ml-auto shrink-0 truncate font-mono text-xs text-muted-foreground">
								{problem.path}:{line + 1}:{character + 1}
							</span>
						</button>
					</li>
				{/each}
			</ul>
		{:else}
			<p class="px-4 py-2 text-sm/6 text-muted-foreground">No problems.</p>
		{/if}
	</ScrollArea>
</section>
