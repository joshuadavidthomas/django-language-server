/// <reference lib="webworker" />
import type { Diagnostic } from "../lsp";
import { createWorkspace, type Workspace } from "./djls";
import type { DjlsSettings, Method, Methods, Request, Response } from "./protocol";

let workspace: Workspace | null = null;
let settingsPath = "";
const paths = new Set<string>();
// Django Discovery reruns lazily, before the next query, so a burst of edits to
// settings or a burst of file additions only rediscovers once.
let needsDiscovery = true;
let discoveryError: string | null = null;

function settingsModulePath(settings: DjlsSettings): string {
  return `${settings.django_settings_module.replaceAll(".", "/")}.py`;
}

function requireWorkspace(): Workspace {
  if (!workspace) throw new Error("workspace is not open");
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

const handlers: { [M in Method]: (params: Methods[M]["params"]) => Promise<Methods[M]["result"]> | Methods[M]["result"] } = {
  async open({ files, settings }) {
    const started = performance.now();
    const ws = await createWorkspace(settings);
    settingsPath = settingsModulePath(settings);
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
  completions: ({ path, line, character }) => JSON.parse(current().completions(path, line, character)),
  hover: ({ path, line, character }) => JSON.parse(current().hover(path, line, character)),
  definition: ({ path, line, character }) => JSON.parse(current().definition(path, line, character)),
  foldingRanges: ({ path }) => JSON.parse(current().foldingRanges(path)),
  readFile: ({ path }) => requireWorkspace().readFile(path),
};

async function handle({ id, method, params }: Request) {
  let response: Response;
  try {
    const handler = handlers[method] as (params: unknown) => unknown;
    response = { id, ok: true, result: await handler(params) };
  } catch (error) {
    response = { id, ok: false, error: String(error), fatal: error instanceof WebAssembly.RuntimeError };
  }
  self.postMessage(response);
}

// `open` is async, so chain requests to keep them strictly ordered behind it.
let queue = Promise.resolve();
self.onmessage = (event: MessageEvent<Request>) => {
  queue = queue.then(() => handle(event.data));
};
