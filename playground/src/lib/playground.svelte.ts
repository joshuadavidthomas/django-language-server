import { createContext, untrack } from 'svelte';
import { toast } from 'svelte-sonner';
import { DjlsClient, WorkerCrashed } from '#lib/djls/client.js';
import type { Diagnostic } from '#lib/djls/lsp.js';
import type { Files } from '#lib/djls/worker/protocol.js';
import { SITE_PACKAGES } from '#lib/djls/worker/pypi.js';
import {
	clearSavedProject,
	DJANGO,
	EXAMPLE_PROJECT,
	languageFor,
	loadProject,
	saveProject,
	SETTINGS
} from '#lib/project.js';
import { projectPath, projectUri, registerProviders, toMarker } from '#lib/monaco/providers.js';
import { monaco } from '#lib/monaco/setup.js';

const MARKER_OWNER = 'djls';
const CHECK_DELAY_MS = 75;
const START_FILE = 'blog/templates/blog/post_list.html';

export type Status =
	| { state: 'starting' }
	| { state: 'ready'; checkMs: number }
	| { state: 'failed'; message: string };

/** A PyPI requirement in the project and how its install went. */
export interface Package {
	requirement: string;
	status: 'installing' | 'installed' | 'failed';
	/** The wheel installed, or why it failed. */
	detail?: string;
}

/** PEP 503's normalized name, so `Django_Crispy_Forms` and `django-crispy-forms` match. */
export function packageName(requirement: string) {
	return requirement
		.split('==')[0]
		.trim()
		.toLowerCase()
		.replace(/[-_.]+/g, '-');
}

export interface Problem {
	path: string;
	diagnostic: Diagnostic;
}

/**
 * The playground: the project's files, djls in its worker, and the Monaco editor over them.
 * Files live here and in localStorage; Monaco keeps one model per file, created when first needed.
 */
export class Playground {
	#saved = loadProject();
	files = $state<Files>(this.#saved.files);
	packages = $state<Package[]>(
		this.#saved.packages.map((requirement) => ({ requirement, status: 'installing' }))
	);
	diagnostics = $state.raw<Record<string, Diagnostic[]>>({});
	/** Project-relative path of the open file; `null` while a bundled Django file is open. */
	current = $state<string | null>(null);
	/** What the editor shows, for the header: the project path, or a read-only module path. */
	label = $state('');
	/** The read-only file's full path, while one is open. */
	readOnlyPath = $state<string | null>(null);
	status = $state<Status>({ state: 'starting' });
	discoveryError = $state<string | null>(null);
	startupMs = $state(0);

	paths = $derived(Object.keys(this.files).sort());
	problems = $derived(
		this.paths.flatMap((path) =>
			(this.diagnostics[path] ?? []).map((diagnostic) => ({ path, diagnostic }))
		)
	);

	#client!: DjlsClient;
	#editor: monaco.editor.IStandaloneCodeEditor | null = null;
	// Keyed by URI string so bundled Django files keep their scroll position too.
	#viewStates = new Map<string, monaco.editor.ICodeEditorViewState | null>();
	#checkTimer: ReturnType<typeof setTimeout> | undefined;
	#checkGeneration = 0;

	/**
	 * Mounts the editor and starts djls; for `{@attach}`. Untracked: an attachment reruns when
	 * state it reads changes, and every edit changes `files`.
	 */
	attach = (element: HTMLElement) => untrack(() => this.#mount(element));

	#mount(element: HTMLElement) {
		this.#client = new DjlsClient((error) => void this.#recover(error));
		const editor = monaco.editor.create(element, {
			automaticLayout: true,
			minimap: { enabled: false },
			fontSize: 13,
			scrollBeyondLastLine: false,
			fixedOverflowWidgets: true
		});
		this.#editor = editor;
		// On a phone, a horizontally scrolled editor hides the start of every line (a tap near the
		// right edge is enough to scroll it), so narrow screens wrap instead. 768px matches the
		// sidebar's mobile breakpoint.
		const narrow = window.matchMedia('(max-width: 767px)');
		const wrap = () => editor.updateOptions({ wordWrap: narrow.matches ? 'on' : 'off' });
		wrap();
		narrow.addEventListener('change', wrap);
		const disposables = [
			{ dispose: () => narrow.removeEventListener('change', wrap) },
			...registerProviders(this.#client, (uri) => this.#ensureModel(uri)),
			// Go to Definition and peek's "open" land here instead of in a new editor.
			monaco.editor.registerEditorOpener({
				openCodeEditor: (_source, resource, selection) => {
					if (!monaco.editor.getModel(resource)) return false;
					this.#openUri(resource, selection);
					return true;
				}
			})
		];
		const files = Object.keys(this.files);
		this.open(files.includes(START_FILE) ? START_FILE : files[0]);
		void this.#start();
		return () => {
			for (const disposable of disposables) disposable.dispose();
			editor.dispose();
			for (const model of monaco.editor.getModels()) model.dispose();
			this.#client.dispose();
			this.#editor = null;
		};
	}

	open(path: string, selection?: monaco.IRange) {
		this.#openUri(projectUri(path), selection);
	}

	/** Opens the location of a diagnostic in its file. */
	reveal({ path, diagnostic }: Problem) {
		this.open(path, toMarker(diagnostic));
	}

	add(path: string) {
		this.files[path] = '';
		void this.#client.call('setFile', { path, content: '' }).catch(ignoreCrash);
		this.#save();
		this.open(path);
		this.#scheduleCheck();
	}

	remove(path: string) {
		delete this.files[path];
		const diagnostics = { ...this.diagnostics };
		delete diagnostics[path];
		this.diagnostics = diagnostics;
		monaco.editor.getModel(projectUri(path))?.dispose();
		this.#viewStates.delete(projectUri(path).toString());
		void this.#client.call('removeFile', { path }).catch(ignoreCrash);
		this.#save();
		if (this.current === path) this.open(this.paths[0]);
		this.#scheduleCheck();
	}

	/** Discards every edit and restores the example project in a fresh worker. */
	async reset() {
		clearSavedProject();
		for (const model of monaco.editor.getModels()) model.dispose();
		this.#viewStates.clear();
		const example = structuredClone(EXAMPLE_PROJECT);
		this.files = example.files;
		this.packages = example.packages.map((requirement) => ({ requirement, status: 'installing' }));
		this.diagnostics = {};
		this.#client.restart();
		this.open(START_FILE);
		await this.#start();
	}

	/** Adds a requirement (`name` or `name==version`); returns why it can't be added, if so. */
	addPackage(requirement: string): string | null {
		const name = packageName(requirement);
		if (!name) return 'Enter a package name.';
		if (this.packages.some((entry) => packageName(entry.requirement) === name)) {
			return `${name} is already in the list.`;
		}
		this.packages.push({ requirement, status: 'installing' });
		this.#save();
		void this.#install(this.packages[this.packages.length - 1]).then(() => this.#scheduleCheck());
		return null;
	}

	removePackage(requirement: string) {
		this.packages = this.packages.filter((entry) => entry.requirement !== requirement);
		this.#save();
		void this.#client.call('removePackage', { requirement }).catch(ignoreCrash);
		this.#scheduleCheck();
	}

	async #install(entry: Package) {
		entry.status = 'installing';
		entry.detail = undefined;
		try {
			const installed = await this.#client.call('installPackage', {
				requirement: entry.requirement
			});
			entry.status = 'installed';
			entry.detail = `${installed.filename} · ${installed.files} files`;
		} catch (cause) {
			if (cause instanceof WorkerCrashed) return;
			entry.status = 'failed';
			entry.detail = cause instanceof Error ? cause.message : String(cause);
		}
	}

	theme(dark: boolean) {
		monaco.editor.setTheme(dark ? 'djls-dark' : 'djls-light');
	}

	async #start() {
		this.status = { state: 'starting' };
		try {
			const opened = await this.#client.call('open', {
				files: $state.snapshot(this.files),
				settings: SETTINGS,
				django: DJANGO
			});
			this.startupMs = opened.startupMs;
			// Queued ahead of the first check, so it already sees the packages.
			for (const entry of this.packages) void this.#install(entry);
		} catch (cause) {
			if (cause instanceof WorkerCrashed) return;
			this.status = { state: 'failed', message: `Failed to start djls: ${cause}` };
			return;
		}
		await this.#check();
	}

	async #recover(error: string) {
		toast.error('djls crashed and is restarting', { description: error });
		this.#client.restart();
		await this.#start();
	}

	#save() {
		saveProject({
			files: $state.snapshot(this.files),
			packages: this.packages.map((entry) => entry.requirement)
		});
	}

	#modelFor(path: string) {
		return monaco.editor.getModel(projectUri(path)) ?? this.#createModel(path);
	}

	#createModel(path: string) {
		const model = monaco.editor.createModel(this.files[path], languageFor(path), projectUri(path));
		model.onDidChangeContent(() => {
			const content = model.getValue();
			this.files[path] = content;
			// Sent on every change so completions and hovers always see current text.
			void this.#client.call('setFile', { path, content }).catch(ignoreCrash);
			this.#scheduleCheck();
			this.#save();
		});
		return model;
	}

	/** Definition targets outside the project are bundled Django sources, shown read-only. */
	async #ensureModel(uri: monaco.Uri) {
		if (monaco.editor.getModel(uri)) return;
		const path = projectPath(uri);
		if (path !== null && path in this.files) {
			this.#modelFor(path);
			return;
		}
		const source = await this.#client.call('readFile', { path: uri.path });
		if (!monaco.editor.getModel(uri)) monaco.editor.createModel(source, languageFor(uri.path), uri);
	}

	#openUri(uri: monaco.Uri, selection?: monaco.IRange | monaco.IPosition) {
		const editor = this.#editor;
		if (!editor) return;
		const previous = editor.getModel();
		if (previous) this.#viewStates.set(previous.uri.toString(), editor.saveViewState());
		const path = projectPath(uri);
		this.current = path !== null && path in this.files ? path : null;
		editor.setModel(
			this.current === null ? monaco.editor.getModel(uri) : this.#modelFor(this.current)
		);
		editor.updateOptions({ readOnly: this.current === null });
		editor.restoreViewState(this.#viewStates.get(uri.toString()) ?? null);
		if (selection) {
			const range = 'lineNumber' in selection ? monaco.Range.fromPositions(selection) : selection;
			editor.setSelection(range);
			editor.revealRangeInCenterIfOutsideViewport(range);
		}
		editor.focus();
		this.label = this.current ?? modulePath(uri.path);
		this.readOnlyPath = this.current === null ? uri.path : null;
	}

	#scheduleCheck() {
		clearTimeout(this.#checkTimer);
		this.#checkTimer = setTimeout(() => void this.#check(), CHECK_DELAY_MS);
	}

	async #check() {
		const generation = ++this.#checkGeneration;
		let result;
		try {
			result = await this.#client.call('diagnostics', {});
		} catch (cause) {
			if (cause instanceof WorkerCrashed) return;
			this.status = { state: 'failed', message: `djls error: ${cause}` };
			return;
		}
		// A newer check is already queued behind this one.
		if (generation !== this.#checkGeneration) return;
		this.diagnostics = result.files;
		for (const path of this.paths) {
			const markers = (result.files[path] ?? []).map(toMarker);
			monaco.editor.setModelMarkers(this.#modelFor(path), MARKER_OWNER, markers);
		}
		this.discoveryError = result.discoveryError;
		this.status = { state: 'ready', checkMs: result.elapsedMs };
	}
}

/**
 * Where a read-only file comes from, and its path as a module: bundled Django is under
 * `/djls-cache/django/<hash>/`, installed packages under the project's site-packages.
 */
export function readOnlySource(path: string) {
	return path.startsWith(`/project/${SITE_PACKAGES}`) ? 'Installed package' : 'Bundled Django';
}

function modulePath(path: string) {
	return path.replace(/^\/djls-cache\/[^/]+\/[^/]+\//, '').replace(`/project/${SITE_PACKAGES}`, '');
}

function ignoreCrash(cause: Error) {
	if (!(cause instanceof WorkerCrashed)) throw cause;
}

export const [getPlayground, setPlayground] = createContext<Playground>();
