import type { CompletionItem, Diagnostic, FoldingRange, Hover, LocationLink } from "../lsp";

/** Project-relative path to file contents. */
export type Files = Record<string, string>;

/** The same JSON shape as `[tool.djls]`. */
export interface DjlsSettings {
  django_settings_module: string;
}

/** A zero-based UTF-16 position in a project file. */
export interface FilePosition {
  path: string;
  line: number;
  character: number;
}

export interface Methods {
  open: { params: { files: Files; settings: DjlsSettings }; result: { startupMs: number } };
  setFile: { params: { path: string; content: string }; result: void };
  removeFile: { params: { path: string }; result: void };
  /** Diagnostics for every project file, plus a project-level error if Django Discovery failed. */
  diagnostics: {
    params: Record<string, never>;
    result: { files: Record<string, Diagnostic[]>; discoveryError: string | null; elapsedMs: number };
  };
  completions: { params: FilePosition; result: CompletionItem[] };
  hover: { params: FilePosition; result: Hover | null };
  /** Targets are `file://` URIs: `/project/...` for project files, `/djls-cache/...` for bundled Django. */
  definition: { params: FilePosition; result: LocationLink[] };
  foldingRanges: { params: { path: string }; result: FoldingRange[] };
  /** Source of a project-relative path, or of an absolute path such as a bundled definition target. */
  readFile: { params: { path: string }; result: string };
}

export type Method = keyof Methods;

export type Request = {
  [M in Method]: { id: number; method: M; params: Methods[M]["params"] };
}[Method];

export type Response =
  | { id: number; ok: true; result: unknown }
  // `fatal` means the wasm instance trapped (a Rust panic) and must be replaced.
  | { id: number; ok: false; error: string; fatal: boolean };
