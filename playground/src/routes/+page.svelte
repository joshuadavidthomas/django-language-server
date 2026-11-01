<script lang="ts">
	import FileTree from '#lib/components/file-tree.svelte';
	import ModeToggle from '#lib/components/mode-toggle.svelte';
	import NewFileDialog from '#lib/components/new-file-dialog.svelte';
	import Problems from '#lib/components/problems.svelte';
	import * as AlertDialog from '#lib/components/ui/alert-dialog/index.js';
	import { Badge } from '#lib/components/ui/badge/index.js';
	import { Button } from '#lib/components/ui/button/index.js';
	import * as Resizable from '#lib/components/ui/resizable/index.js';
	import * as Sidebar from '#lib/components/ui/sidebar/index.js';
	import { Playground, readOnlySource, setPlayground } from '#lib/playground.svelte.js';
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';
	import RotateCcw from '@lucide/svelte/icons/rotate-ccw';
	import { mode } from 'mode-watcher';

	const playground = setPlayground(new Playground());

	let creating = $state(false);
	// bits-ui's `AlertDialog.Action` doesn't close its dialog, so the page does.
	let resetting = $state(false);

	$effect(() => playground.theme(mode.current === 'dark'));

	const summary = $derived.by(() => {
		const { status, problems, startupMs } = playground;
		if (status.state !== 'ready') return null;
		const count = `${problems.length} ${problems.length === 1 ? 'problem' : 'problems'}`;
		return `${count} · checked in ${status.checkMs.toFixed(1)} ms · started in ${startupMs.toFixed(0)} ms`;
	});
</script>

<Sidebar.Provider class="isolate h-svh" style="--header-height: calc(var(--spacing) * 12);">
	<FileTree onNewFile={() => (creating = true)} />
	<Sidebar.Inset class="min-h-0 min-w-0">
		<header class="flex h-(--header-height) shrink-0 items-center gap-2 border-b px-4">
			<Sidebar.Trigger class="mr-1 -ml-1" />
			<div class="flex min-w-0 items-center gap-2">
				<h1 class="truncate font-mono text-sm/6" data-testid="open-file">{playground.label}</h1>
				{#if playground.readOnlyPath}
					<Badge variant="secondary">{readOnlySource(playground.readOnlyPath)} · read-only</Badge>
				{/if}
			</div>
			<div class="ml-auto flex min-w-0 items-center gap-2">
				<p
					class={[
						'truncate text-sm/6 max-md:hidden',
						playground.status.state === 'failed' || playground.discoveryError
							? 'text-destructive'
							: 'text-muted-foreground'
					]}
					role="status"
					data-testid="status"
				>
					{#if playground.status.state === 'starting'}
						<LoaderCircle class="mr-1 inline size-4 animate-spin align-[-3px]" />Starting djls…
					{:else if playground.status.state === 'failed'}
						{playground.status.message}
					{:else if playground.discoveryError}
						Django Discovery failed: {playground.discoveryError}
					{:else}
						{summary}
					{/if}
				</p>
				<AlertDialog.Root bind:open={resetting}>
					<AlertDialog.Trigger>
						{#snippet child({ props })}
							<Button {...props} variant="outline" size="sm"><RotateCcw />Reset</Button>
						{/snippet}
					</AlertDialog.Trigger>
					<AlertDialog.Content>
						<AlertDialog.Header>
							<AlertDialog.Title>Restore the example project?</AlertDialog.Title>
							<AlertDialog.Description>
								Your edits and new files are discarded.
							</AlertDialog.Description>
						</AlertDialog.Header>
						<AlertDialog.Footer>
							<AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
							<AlertDialog.Action
								variant="destructive"
								onclick={() => {
									resetting = false;
									void playground.reset();
								}}
							>
								Reset
							</AlertDialog.Action>
						</AlertDialog.Footer>
					</AlertDialog.Content>
				</AlertDialog.Root>
				<ModeToggle />
			</div>
		</header>
		<Resizable.PaneGroup direction="vertical" class="min-h-0 flex-1">
			<Resizable.Pane defaultSize={78} minSize={30}>
				<div class="h-full" {@attach playground.attach}></div>
			</Resizable.Pane>
			<Resizable.Handle />
			<Resizable.Pane defaultSize={22} minSize={8}>
				<Problems />
			</Resizable.Pane>
		</Resizable.PaneGroup>
	</Sidebar.Inset>
</Sidebar.Provider>
<NewFileDialog bind:open={creating} />
