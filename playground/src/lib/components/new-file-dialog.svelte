<script lang="ts">
	import { Button } from '#lib/components/ui/button/index.js';
	import * as Dialog from '#lib/components/ui/dialog/index.js';
	import * as Field from '#lib/components/ui/field/index.js';
	import { Input } from '#lib/components/ui/input/index.js';
	import { getPlayground } from '#lib/playground.svelte.js';
	import { untrack } from 'svelte';

	let { open = $bindable(false) }: { open?: boolean } = $props();

	const playground = getPlayground();

	/** Starts in the open file's folder, where a new file usually goes. */
	let path = $state('');
	const trimmed = $derived(path.trim().replace(/^\/+/, ''));
	const error = $derived.by(() => {
		if (!trimmed) return null;
		if (trimmed.endsWith('/')) return 'Add a file name after the folder.';
		if (trimmed in playground.files) return `${trimmed} already exists.`;
		return null;
	});

	// The tree opens this by setting `open`, which `onOpenChange` doesn't report.
	$effect(() => {
		if (!open) return;
		const current = untrack(() => playground.current) ?? '';
		path = current.includes('/') ? current.slice(0, current.lastIndexOf('/') + 1) : '';
	});

	function submit(event: SubmitEvent) {
		event.preventDefault();
		if (!trimmed || error) return;
		playground.add(trimmed);
		open = false;
	}
</script>

<Dialog.Root bind:open>
	<!-- Closing would focus the tree's button; the editor has already taken focus for the new file. -->
	<Dialog.Content class="sm:max-w-md" onCloseAutoFocus={(event) => event.preventDefault()}>
		<form onsubmit={submit} class="flex flex-col gap-6">
			<Dialog.Header>
				<Dialog.Title>New file</Dialog.Title>
				<Dialog.Description>
					A new app or templatetags library is picked up as soon as it exists.
				</Dialog.Description>
			</Dialog.Header>
			<Field.Field data-invalid={error ? true : undefined}>
				<Field.Label for="new-file-path">Path</Field.Label>
				<Input
					id="new-file-path"
					bind:value={path}
					placeholder="blog/templatetags/extras.py"
					autocomplete="off"
					spellcheck={false}
					aria-invalid={error ? true : undefined}
					class="font-mono"
				/>
				{#if error}
					<Field.Error>{error}</Field.Error>
				{:else}
					<Field.Description>Relative to the project root.</Field.Description>
				{/if}
			</Field.Field>
			<Dialog.Footer>
				<Dialog.Close>
					{#snippet child({ props })}
						<Button {...props} variant="outline">Cancel</Button>
					{/snippet}
				</Dialog.Close>
				<Button type="submit" disabled={!trimmed || !!error}>Create</Button>
			</Dialog.Footer>
		</form>
	</Dialog.Content>
</Dialog.Root>
