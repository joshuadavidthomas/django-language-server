<script lang="ts">
	import { Badge } from '#lib/components/ui/badge/index.js';
	import { ScrollArea } from '#lib/components/ui/scroll-area/index.js';
	import { getPlayground } from '#lib/playground.svelte.js';
	import CircleX from '@lucide/svelte/icons/circle-x';
	import Info from '@lucide/svelte/icons/info';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';

	const playground = getPlayground();
</script>

<section class="@container flex h-full min-h-0 flex-col" aria-labelledby="problems-title">
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
						<!-- Narrow panels put the message first and let it wrap, with the code and file below. -->
						<button
							type="button"
							class="flex w-full items-start gap-2 px-4 py-1 text-left text-sm/6 hover:bg-muted"
							title="{problem.path}:{line + 1}:{character + 1}"
							onclick={() => playground.reveal(problem)}
						>
							{#if severity === 1}
								<CircleX class="mt-1 size-4 shrink-0 text-destructive" aria-label="Error" />
							{:else if severity === 2}
								<TriangleAlert
									class="mt-1 size-4 shrink-0 text-amber-600 dark:text-amber-400"
									aria-label="Warning"
								/>
							{:else}
								<Info class="mt-1 size-4 shrink-0 text-muted-foreground" aria-label="Info" />
							{/if}
							<span
								class="flex min-w-0 flex-1 flex-col @2xl:flex-row @2xl:items-baseline @2xl:gap-2"
							>
								<span class="break-words @2xl:truncate">{problem.diagnostic.message}</span>
								<span
									class="flex min-w-0 items-baseline gap-2 font-mono text-xs/5 text-muted-foreground @2xl:contents"
								>
									{#if problem.diagnostic.code !== undefined}
										<span class="shrink-0">{problem.diagnostic.code}</span>
									{/if}
									<span class="truncate @2xl:ml-auto @2xl:shrink-0">
										<span class="@2xl:hidden">{problem.path.split('/').at(-1)}</span><span
											class="@max-2xl:hidden">{problem.path}</span
										>:{line + 1}:{character + 1}
									</span>
								</span>
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
