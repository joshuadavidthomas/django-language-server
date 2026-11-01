<script lang="ts">
	import { Button } from '#lib/components/ui/button/index.js';
	import * as Sidebar from '#lib/components/ui/sidebar/index.js';
	import { getPlayground } from '#lib/playground.svelte.js';
	import CircleX from '@lucide/svelte/icons/circle-x';
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';
	import PackageIcon from '@lucide/svelte/icons/package';
	import Plus from '@lucide/svelte/icons/plus';
	import Trash2 from '@lucide/svelte/icons/trash-2';

	const playground = getPlayground();

	let requirement = $state('');
	let error = $state<string | null>(null);

	function submit(event: SubmitEvent) {
		event.preventDefault();
		error = playground.addPackage(requirement.trim());
		if (!error) requirement = '';
	}
</script>

<Sidebar.Group>
	<Sidebar.GroupLabel>Packages</Sidebar.GroupLabel>
	<Sidebar.GroupContent class="flex flex-col gap-2">
		<form class="flex gap-1 px-1" onsubmit={submit}>
			<Sidebar.Input
				bind:value={requirement}
				oninput={() => (error = null)}
				placeholder="django-allauth==65.3.0"
				aria-label="PyPI requirement"
				aria-invalid={error ? true : undefined}
				autocomplete="off"
				spellcheck={false}
				class="font-mono text-xs"
			/>
			<Button type="submit" variant="outline" size="icon-sm" aria-label="Install package">
				<Plus />
			</Button>
		</form>
		{#if error}
			<p class="px-2 text-xs/5 text-destructive" role="alert">{error}</p>
		{/if}
		<Sidebar.Menu>
			{#each playground.packages as entry (entry.requirement)}
				<Sidebar.MenuItem data-package={entry.requirement} data-status={entry.status}>
					<div class="flex min-w-0 items-start gap-2 rounded-md px-2 py-1.5 pr-8 text-sm">
						{#if entry.status === 'installing'}
							<LoaderCircle
								class="mt-0.5 size-4 shrink-0 animate-spin text-muted-foreground"
								aria-label="Installing"
							/>
						{:else if entry.status === 'installed'}
							<PackageIcon class="mt-0.5 size-4 shrink-0 text-brand" aria-label="Installed" />
						{:else}
							<CircleX class="mt-0.5 size-4 shrink-0 text-destructive" aria-label="Failed" />
						{/if}
						<div class="flex min-w-0 flex-col">
							<span class="truncate font-mono text-xs/5">{entry.requirement}</span>
							{#if entry.detail}
								<span
									class={[
										'text-xs/4 break-words',
										entry.status === 'failed' ? 'text-destructive' : 'text-muted-foreground'
									]}
								>
									{entry.detail}
								</span>
							{/if}
						</div>
					</div>
					<Sidebar.MenuAction
						showOnHover
						onclick={() => playground.removePackage(entry.requirement)}
						title="Remove {entry.requirement}"
					>
						<Trash2 />
					</Sidebar.MenuAction>
				</Sidebar.MenuItem>
			{:else}
				<p class="px-2 text-xs/5 text-muted-foreground">
					No packages. Each needs a pure-Python wheel on PyPI; dependencies aren't installed.
				</p>
			{/each}
		</Sidebar.Menu>
	</Sidebar.GroupContent>
</Sidebar.Group>
