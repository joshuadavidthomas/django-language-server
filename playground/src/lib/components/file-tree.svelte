<script lang="ts">
	import Packages from '#lib/components/packages.svelte';
	import * as AlertDialog from '#lib/components/ui/alert-dialog/index.js';
	import * as Collapsible from '#lib/components/ui/collapsible/index.js';
	import * as Sidebar from '#lib/components/ui/sidebar/index.js';
	import { getPlayground } from '#lib/playground.svelte.js';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import FileCode from '@lucide/svelte/icons/file-code';
	import FileText from '@lucide/svelte/icons/file-text';
	import Folder from '@lucide/svelte/icons/folder';
	import Plus from '@lucide/svelte/icons/plus';
	import Trash2 from '@lucide/svelte/icons/trash-2';

	/** `onNewFile` opens the new-file dialog, which the page owns. */
	let { onNewFile }: { onNewFile: () => void } = $props();

	const playground = getPlayground();

	interface Directory {
		name: string;
		path: string;
		dirs: Directory[];
		files: string[];
	}

	/** The paths as nested folders, folders first, each level sorted by name. */
	const tree = $derived.by(() => {
		const root: Directory = { name: '', path: '', dirs: [], files: [] };
		for (const path of playground.paths) {
			let node = root;
			for (const name of path.split('/').slice(0, -1)) {
				const dirPath = node.path ? `${node.path}/${name}` : name;
				let child = node.dirs.find((dir) => dir.name === name);
				if (!child) {
					child = { name, path: dirPath, dirs: [], files: [] };
					node.dirs.push(child);
					node.dirs.sort((a, b) => a.name.localeCompare(b.name));
				}
				node = child;
			}
			node.files.push(path);
		}
		return root;
	});

	/** The file waiting on the delete confirmation. */
	let deleting = $state<string | null>(null);

	function counts(path: string) {
		const diagnostics = playground.diagnostics[path] ?? [];
		return {
			errors: diagnostics.filter((diagnostic) => (diagnostic.severity ?? 1) === 1).length,
			warnings: diagnostics.filter((diagnostic) => diagnostic.severity === 2).length
		};
	}
</script>

{#snippet directory(dir: Directory)}
	{#each dir.dirs as sub (sub.path)}
		<Sidebar.MenuItem>
			<Collapsible.Root
				open
				class="group/collapsible [&[data-state=open]>button>svg:first-child]:rotate-90"
			>
				<Collapsible.Trigger>
					{#snippet child({ props })}
						<Sidebar.MenuButton {...props}>
							<ChevronRight class="transition-transform" />
							<Folder />
							<span>{sub.name}</span>
						</Sidebar.MenuButton>
					{/snippet}
				</Collapsible.Trigger>
				<Collapsible.Content>
					<Sidebar.MenuSub class="mr-0 pr-0">
						{@render directory(sub)}
					</Sidebar.MenuSub>
				</Collapsible.Content>
			</Collapsible.Root>
		</Sidebar.MenuItem>
	{/each}
	{#each dir.files as path (path)}
		{@const { errors, warnings } = counts(path)}
		{@const name = path.split('/').at(-1)}
		<Sidebar.MenuItem>
			<Sidebar.MenuButton
				isActive={playground.current === path}
				onclick={() => playground.open(path)}
				data-path={path}
				title={path}
			>
				{#if path.endsWith('.py')}<FileCode />{:else}<FileText />{/if}
				<span>{name}</span>
			</Sidebar.MenuButton>
			{#if errors || warnings}
				<!-- Gives way to the delete action on hover. -->
				<Sidebar.MenuBadge
					class={[
						'group-hover/menu-item:opacity-0',
						errors ? 'text-destructive' : 'text-amber-600 dark:text-amber-400'
					]}
					aria-label="{errors + warnings} problems"
				>
					{errors + warnings}
				</Sidebar.MenuBadge>
			{/if}
			<Sidebar.MenuAction showOnHover onclick={() => (deleting = path)} title="Delete {path}">
				<Trash2 />
			</Sidebar.MenuAction>
		</Sidebar.MenuItem>
	{/each}
{/snippet}

<Sidebar.Root>
	<Sidebar.Header>
		<div class="flex items-center gap-2 px-2 py-1">
			<div
				class="flex size-8 shrink-0 items-center justify-center rounded-lg bg-brand text-sm font-semibold text-white"
			>
				dj
			</div>
			<div class="flex min-w-0 flex-col">
				<span class="truncate text-sm/5 font-semibold">django-language-server</span>
				<span class="truncate text-xs/4 text-muted-foreground">Playground</span>
			</div>
		</div>
	</Sidebar.Header>
	<Sidebar.Content>
		<Sidebar.Group>
			<Sidebar.GroupLabel>Project</Sidebar.GroupLabel>
			<Sidebar.GroupAction title="New file" onclick={onNewFile}>
				<Plus />
				<span class="sr-only">New file</span>
			</Sidebar.GroupAction>
			<Sidebar.GroupContent>
				<Sidebar.Menu>
					{@render directory(tree)}
				</Sidebar.Menu>
			</Sidebar.GroupContent>
		</Sidebar.Group>
		<Packages />
	</Sidebar.Content>
	<Sidebar.Rail />
</Sidebar.Root>

<AlertDialog.Root
	open={deleting !== null}
	onOpenChange={(open) => {
		if (!open) deleting = null;
	}}
>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>Delete {deleting}?</AlertDialog.Title>
			<AlertDialog.Description>
				Reset restores the example project's files, not your edits.
			</AlertDialog.Description>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
			<AlertDialog.Action
				variant="destructive"
				onclick={() => {
					if (deleting) playground.remove(deleting);
					deleting = null;
				}}
			>
				Delete
			</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
