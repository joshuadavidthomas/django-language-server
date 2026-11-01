// The only module that touches the generated wasm-bindgen package or the Django sources it
// loads. If how the wasm or Django gets fetched changes, change it here.
import init, { registerDjango, Workspace } from '../../../../djls-wasm/djls_wasm.js';
import type { DjangoRelease, DjlsSettings } from '#lib/djls/worker/protocol.js';

export type { Workspace };

// Written by `just playground wasm`: each vendored release, stored uncompressed so HTTP
// compression applies. Picking a release is picking one of these URLs.
const djangoUrls = import.meta.glob<string>('../../../../djls-wasm/django-*.zip', {
	query: '?url',
	import: 'default',
	eager: true
});

let ready: Promise<void> | undefined;

/**
 * Instantiates the wasm and registers one Django release, fetching both in parallel. Once per
 * worker: a release is registered before any Workspace exists, so switching releases needs a
 * new worker.
 */
function load(django: DjangoRelease) {
	ready ??= (async () => {
		const url = djangoUrls[`../../../../djls-wasm/django-${django}.zip`];
		if (!url) throw new Error(`Django ${django} was not built; run \`just playground wasm\``);
		const [, response] = await Promise.all([init(), fetch(url)]);
		if (!response.ok) throw new Error(`Failed to fetch Django ${django}: ${response.status}`);
		registerDjango(new Uint8Array(await response.arrayBuffer()));
	})();
	return ready;
}

export async function createWorkspace(settings: DjlsSettings, django: DjangoRelease) {
	await load(django);
	return new Workspace(JSON.stringify(settings));
}
