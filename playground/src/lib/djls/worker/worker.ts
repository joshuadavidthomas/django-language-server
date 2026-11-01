/// <reference lib="webworker" />
import type { Diagnostic } from '#lib/djls/lsp.js';
import { createWorkspace, type Workspace } from '#lib/djls/worker/djls.js';
import { PYVENV_CFG, resolveWheel, unpackWheel } from '#lib/djls/worker/pypi.js';
import type {
	DjlsSettings,
	Handlers,
	Method,
	Request,
	RequestOf,
	Response
} from '#lib/djls/worker/protocol.js';

let workspace: Workspace | null = null;
let settingsPath = '';
const paths = new Set<string>();
// Each installed requirement's site-packages files, so removing it can delete them. Kept out of
// `paths`: diagnostics cover the user's files, not the packages'.
const packages = new Map<string, string[]>();
// Django Discovery reruns lazily, before the next query, so a burst of edits to
// settings or a burst of file additions only rediscovers once.
let needsDiscovery = true;
let discoveryError: string | null = null;

function settingsModulePath(settings: DjlsSettings): string {
	return `${settings.django_settings_module.replaceAll('.', '/')}.py`;
}

function requireWorkspace(): Workspace {
	if (!workspace) throw new Error('workspace is not open');
	return workspace;
}

/** The workspace, rediscovered first if a settings or structure change is pending. */
function current(): Workspace {
	const ws = requireWorkspace();
	discover(ws);
	return ws;
}

function discover(ws: Workspace) {
	if (!needsDiscovery) return;
	needsDiscovery = false;
	try {
		ws.discover();
		discoveryError = null;
	} catch (error) {
		if (error instanceof WebAssembly.RuntimeError) throw error;
		discoveryError = String(error);
	}
}

function setFile(ws: Workspace, path: string, content: string) {
	// New files can be new apps or templatetags libraries; editing an existing
	// `.py` file invalidates incrementally without rediscovery.
	if (!paths.has(path) || path === settingsPath) needsDiscovery = true;
	paths.add(path);
	ws.setFile(path, content);
}

const handlers: Handlers = {
	async open({ files, settings, django }) {
		const started = performance.now();
		const ws = await createWorkspace(settings, django);
		settingsPath = settingsModulePath(settings);
		// Without a Python environment djls can't prove a library is absent, so unknown-library
		// diagnostics would depend on whether a package was ever installed.
		ws.setFile(PYVENV_CFG.path, PYVENV_CFG.content);
		for (const [path, content] of Object.entries(files)) setFile(ws, path, content);
		discover(ws);
		workspace = ws;
		return { startupMs: performance.now() - started };
	},
	setFile({ path, content }) {
		setFile(requireWorkspace(), path, content);
	},
	removeFile({ path }) {
		requireWorkspace().removeFile(path);
		paths.delete(path);
		needsDiscovery = true;
	},
	diagnostics() {
		const started = performance.now();
		const ws = current();
		const files: Record<string, Diagnostic[]> = {};
		for (const path of paths) files[path] = JSON.parse(ws.diagnostics(path)) as Diagnostic[];
		return { files, discoveryError, elapsedMs: performance.now() - started };
	},
	completions: ({ path, line, character }) =>
		JSON.parse(current().completions(path, line, character)),
	hover: ({ path, line, character }) => JSON.parse(current().hover(path, line, character)),
	definition: ({ path, line, character }) =>
		JSON.parse(current().definition(path, line, character)),
	foldingRanges: ({ path }) => JSON.parse(current().foldingRanges(path)),
	async installPackage({ requirement }) {
		const wheel = await resolveWheel(requirement);
		const files = await unpackWheel(wheel);
		const ws = requireWorkspace();
		for (const [path, content] of files) ws.setFile(path, content);
		packages.set(requirement, [...files.keys()]);
		// Site-packages roots and template libraries are only found by Django Discovery.
		needsDiscovery = true;
		return { filename: wheel.filename, files: files.size };
	},
	removePackage({ requirement }) {
		const ws = requireWorkspace();
		for (const path of packages.get(requirement) ?? []) ws.removeFile(path);
		packages.delete(requirement);
		needsDiscovery = true;
	},
	format: ({ path, tabSize, insertSpaces }) =>
		JSON.parse(current().format(path, tabSize, insertSpaces)),
	readFile: ({ path }) => requireWorkspace().readFile(path)
};

async function handle<M extends Method>({ id, method, params }: RequestOf<M>) {
	let response: Response;
	try {
		response = { id, ok: true, result: await handlers[method](params) };
	} catch (error) {
		response = {
			id,
			ok: false,
			// wasm-bindgen's JsError and our own are Errors; the message is what the UI shows.
			error: error instanceof Error ? error.message : String(error),
			fatal: error instanceof WebAssembly.RuntimeError
		};
	}
	self.postMessage(response);
}

// `open` is async, so chain requests to keep them strictly ordered behind it.
let queue = Promise.resolve();
self.onmessage = (event: MessageEvent<Request>) => {
	queue = queue.then(() => handle(event.data));
};
