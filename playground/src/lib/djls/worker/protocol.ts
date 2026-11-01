import type {
	CompletionItem,
	Diagnostic,
	FoldingRange,
	Hover,
	LocationLink,
	TextEdit
} from '#lib/djls/lsp.js';

/** Project-relative path to file contents. */
export type Files = Record<string, string>;

/** The same JSON shape as `[tool.djls]`. */
export interface DjlsSettings {
	django_settings_module: string;
	format: { enabled: boolean };
}

/** A Django release vendored in `crates/djls-project/vendor/`, which the worker registers. */
export type DjangoRelease = '5.2' | '6.0' | '6.1';

/** A zero-based UTF-16 position in a project file. */
export interface FilePosition {
	path: string;
	line: number;
	character: number;
}

export interface Methods {
	open: {
		params: { files: Files; settings: DjlsSettings; django: DjangoRelease };
		result: { startupMs: number };
	};
	setFile: { params: { path: string; content: string }; result: void };
	removeFile: { params: { path: string }; result: void };
	/** Diagnostics for every project file, plus a project-level error if Django Discovery failed. */
	diagnostics: {
		params: Record<string, never>;
		result: {
			files: Record<string, Diagnostic[]>;
			discoveryError: string | null;
			elapsedMs: number;
		};
	};
	completions: { params: FilePosition; result: CompletionItem[] };
	hover: { params: FilePosition; result: Hover | null };
	/** Targets are `file://` URIs: `/project/...` for project files, `/djls-cache/...` for bundled Django. */
	definition: { params: FilePosition; result: LocationLink[] };
	foldingRanges: { params: { path: string }; result: FoldingRange[] };
	/**
	 * Fetches the requirement's pure-Python wheel from PyPI and unpacks it into the project's
	 * `.venv` site-packages. `name` or `name==version`; dependencies are not resolved.
	 */
	installPackage: { params: { requirement: string }; result: { filename: string; files: number } };
	removePackage: { params: { requirement: string }; result: void };
	format: {
		params: { path: string; tabSize: number; insertSpaces: boolean };
		result: TextEdit[];
	};
	/** Source of a project-relative path, or of an absolute path such as a bundled definition target. */
	readFile: { params: { path: string }; result: string };
}

export type Method = keyof Methods;

export interface RequestOf<M extends Method> {
	id: number;
	method: M;
	params: Methods[M]['params'];
}

export type Request = { [M in Method]: RequestOf<M> }[Method];

export type Result = Methods[Method]['result'];

/** The worker's implementation of each method. */
export type Handlers = {
	[M in Method]: (
		params: Methods[M]['params']
	) => Promise<Methods[M]['result']> | Methods[M]['result'];
};

export type Response =
	| { id: number; ok: true; result: Result }
	// `fatal` means the wasm instance trapped (a Rust panic) and must be replaced.
	| { id: number; ok: false; error: string; fatal: boolean };
