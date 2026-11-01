import "monaco-editor/features/register.all";
// `register.all` leaves out the Go to Definition / Peek commands that `editor.main` adds.
import "monaco-editor/editor/contrib/gotoSymbol/browser/goToCommands";
import "monaco-editor/languages/definitions/python/register";
import * as monaco from "monaco-editor/editor";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";
import { DjlsClient, WorkerCrashed } from "./client";
import * as djangoTemplate from "./django-template";
import type { Diagnostic } from "./lsp";
import { clearSavedFiles, EXAMPLE_FILES, languageFor, loadFiles, saveFiles, SETTINGS } from "./project";
import { projectPath, projectUri, registerProviders, toMarker } from "./providers";
import "./style.css";
import type { Files } from "./worker/protocol";

self.MonacoEnvironment = { getWorker: () => new EditorWorker() };

monaco.languages.register({ id: djangoTemplate.DJANGO_TEMPLATE, aliases: ["Django HTML"] });
monaco.languages.setMonarchTokensProvider(djangoTemplate.DJANGO_TEMPLATE, djangoTemplate.language);
monaco.languages.setLanguageConfiguration(djangoTemplate.DJANGO_TEMPLATE, djangoTemplate.configuration);
monaco.editor.defineTheme("djls-dark", {
  base: "vs-dark",
  inherit: true,
  rules: [
    { token: "delimiter.django", foreground: "e5a04a", fontStyle: "bold" },
    { token: "keyword.django", foreground: "c586c0" },
    { token: "keyword.operator.django", foreground: "569cd6" },
    { token: "function.django", foreground: "dcdcaa" },
    { token: "variable.django", foreground: "9cdcfe" },
    { token: "string.django", foreground: "ce9178" },
    { token: "number.django", foreground: "b5cea8" },
    { token: "constant.django", foreground: "569cd6" },
    { token: "operator.django", foreground: "d4d4d4" },
    { token: "comment.django", foreground: "6a9955", fontStyle: "italic" },
  ],
  colors: {},
});

const MARKER_OWNER = "djls";
const CHECK_DELAY_MS = 75;

const statusElement = document.getElementById("status")!;
const treeElement = document.getElementById("tree")!;
const tabElement = document.getElementById("tab")!;
const problemList = document.getElementById("problem-list")!;

let files: Files = loadFiles();
/** Project-relative path of the open file; `null` while a bundled Django file is open. */
let current: string | null = Object.keys(files).find((path) => path.endsWith("post_list.html")) ?? Object.keys(files)[0];
let diagnostics: Record<string, Diagnostic[]> = {};
let startupMs = 0;
// Keyed by URI string so bundled Django files keep their scroll position too.
const viewStates = new Map<string, monaco.editor.ICodeEditorViewState | null>();

const client = new DjlsClient((error) => void recover(error));
registerProviders(client, ensureModel);

const editor = monaco.editor.create(document.getElementById("editor")!, {
  theme: "djls-dark",
  automaticLayout: true,
  minimap: { enabled: false },
  fontSize: 13,
  scrollBeyondLastLine: false,
  fixedOverflowWidgets: true,
});

function modelFor(path: string) {
  return monaco.editor.getModel(projectUri(path)) ?? createModel(path);
}

function createModel(path: string) {
  const model = monaco.editor.createModel(files[path], languageFor(path), projectUri(path));
  model.onDidChangeContent(() => {
    files[path] = model.getValue();
    // Sent on every change so completions and hovers always see current text.
    void client.call("setFile", { path, content: files[path] }).catch(ignoreCrash);
    scheduleCheck();
    saveFiles(files);
  });
  return model;
}

function setStatus(text: string, isError = false) {
  statusElement.textContent = text;
  statusElement.classList.toggle("error", isError);
}

function ignoreCrash(error: unknown) {
  if (!(error instanceof WorkerCrashed)) throw error;
}

let checkTimer: number | undefined;
let checkGeneration = 0;

function scheduleCheck() {
  clearTimeout(checkTimer);
  checkTimer = window.setTimeout(() => void check(), CHECK_DELAY_MS);
}

async function check() {
  const generation = ++checkGeneration;
  let result;
  try {
    result = await client.call("diagnostics", {});
  } catch (error) {
    if (error instanceof WorkerCrashed) return;
    setStatus(`djls error: ${error}`, true);
    return;
  }
  // A newer check is already queued behind this one.
  if (generation !== checkGeneration) return;
  diagnostics = result.files;
  for (const path of Object.keys(files)) {
    monaco.editor.setModelMarkers(modelFor(path), MARKER_OWNER, (diagnostics[path] ?? []).map(toMarker));
  }
  const count = Object.values(diagnostics).reduce((total, list) => total + list.length, 0);
  if (result.discoveryError) {
    setStatus(`Django Discovery failed: ${result.discoveryError}`, true);
  } else {
    setStatus(
      `${count} ${count === 1 ? "problem" : "problems"} · checked in ${result.elapsedMs.toFixed(1)} ms · started in ${startupMs.toFixed(0)} ms`,
    );
  }
  renderTree();
  renderProblems();
}

/** Definition targets outside the project are bundled Django sources, shown read-only. */
async function ensureModel(uri: monaco.Uri) {
  if (monaco.editor.getModel(uri)) return;
  const path = projectPath(uri);
  if (path !== null && path in files) {
    modelFor(path);
    return;
  }
  const source = await client.call("readFile", { path: uri.path });
  if (!monaco.editor.getModel(uri)) monaco.editor.createModel(source, languageFor(uri.path), uri);
}

function openUri(uri: monaco.Uri, selection?: monaco.IRange | monaco.IPosition) {
  const previous = editor.getModel();
  if (previous) viewStates.set(previous.uri.toString(), editor.saveViewState());
  const path = projectPath(uri);
  current = path !== null && path in files ? path : null;
  editor.setModel(current === null ? monaco.editor.getModel(uri) : modelFor(current));
  editor.updateOptions({ readOnly: current === null });
  editor.restoreViewState(viewStates.get(uri.toString()) ?? null);
  if (selection) {
    const range = "lineNumber" in selection ? monaco.Range.fromPositions(selection) : selection;
    editor.setSelection(range);
    editor.revealRangeInCenterIfOutsideViewport(range);
  }
  editor.focus();
  // Bundled paths look like `/djls-cache/django/<hash>/django/...`; show the module path.
  tabElement.textContent = current ?? `${uri.path.replace(/^\/djls-cache\/[^/]+\/[^/]+\//, "")} (bundled Django, read-only)`;
  renderTree();
}

function openFile(path: string, selection?: monaco.IRange) {
  openUri(projectUri(path), selection);
}

// Go to Definition and peek's "open" land here instead of in a new editor.
monaco.editor.registerEditorOpener({
  openCodeEditor(_source, resource, selection) {
    if (!monaco.editor.getModel(resource)) return false;
    openUri(resource, selection);
    return true;
  },
});

function counts(path: string) {
  const list = diagnostics[path] ?? [];
  return {
    errors: list.filter((diagnostic) => (diagnostic.severity ?? 1) === 1).length,
    warnings: list.filter((diagnostic) => diagnostic.severity === 2).length,
  };
}

function badge(kind: "error" | "warning", count: number) {
  const element = document.createElement("span");
  element.className = `badge ${kind}`;
  element.textContent = String(count);
  element.title = `${count} ${kind}${count === 1 ? "" : "s"}`;
  return element;
}

interface Directory {
  dirs: Map<string, Directory>;
  files: string[];
}

function buildTree(paths: string[]): Directory {
  const root: Directory = { dirs: new Map(), files: [] };
  for (const path of paths) {
    const parts = path.split("/");
    let node = root;
    for (const part of parts.slice(0, -1)) {
      if (!node.dirs.has(part)) node.dirs.set(part, { dirs: new Map(), files: [] });
      node = node.dirs.get(part)!;
    }
    node.files.push(path);
  }
  return root;
}

function renderTree() {
  const rows: HTMLElement[] = [];
  const visit = (node: Directory, depth: number) => {
    for (const [name, child] of [...node.dirs].sort(([a], [b]) => a.localeCompare(b))) {
      const row = document.createElement("div");
      row.className = "tree-dir";
      row.style.paddingLeft = `${8 + depth * 12}px`;
      row.textContent = `${name}/`;
      rows.push(row);
      visit(child, depth + 1);
    }
    for (const path of node.files.sort()) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = `tree-file${path === current ? " active" : ""}`;
      row.style.paddingLeft = `${8 + depth * 12}px`;
      row.dataset.path = path;
      row.onclick = () => openFile(path);
      const name = document.createElement("span");
      name.className = "name";
      name.textContent = path.split("/").at(-1)!;
      row.append(name);
      const { errors, warnings } = counts(path);
      if (errors) row.append(badge("error", errors));
      if (warnings) row.append(badge("warning", warnings));
      const remove = document.createElement("span");
      remove.className = "delete";
      remove.textContent = "×";
      remove.title = `Delete ${path}`;
      remove.onclick = (event) => {
        event.stopPropagation();
        deleteFile(path);
      };
      row.append(remove);
      rows.push(row);
    }
  };
  visit(buildTree(Object.keys(files)), 0);
  treeElement.replaceChildren(...rows);
}

const SEVERITY_NAMES = { 1: "error", 2: "warning", 3: "info", 4: "info" } as const;

function renderProblems() {
  const items: HTMLElement[] = [];
  for (const path of Object.keys(files).sort()) {
    for (const diagnostic of diagnostics[path] ?? []) {
      const item = document.createElement("li");
      const { line, character } = diagnostic.range.start;
      item.innerHTML = `<span class="severity ${SEVERITY_NAMES[diagnostic.severity ?? 1]}"></span><span class="message"></span><span class="code"></span><span class="where"></span>`;
      item.querySelector(".message")!.textContent = diagnostic.message;
      item.querySelector(".code")!.textContent = diagnostic.code === undefined ? "" : `[${diagnostic.code}]`;
      item.querySelector(".where")!.textContent = `${path}:${line + 1}:${character + 1}`;
      item.onclick = () => openFile(path, toMarker(diagnostic));
      items.push(item);
    }
  }
  if (items.length === 0) {
    const empty = document.createElement("li");
    empty.className = "empty";
    empty.textContent = "No problems.";
    items.push(empty);
  }
  problemList.replaceChildren(...items);
}

function addFile(path: string) {
  files[path] = "";
  void client.call("setFile", { path, content: "" }).catch(ignoreCrash);
  saveFiles(files);
  openFile(path);
  scheduleCheck();
}

function deleteFile(path: string) {
  if (!confirm(`Delete ${path}?`)) return;
  delete files[path];
  delete diagnostics[path];
  monaco.editor.getModel(projectUri(path))?.dispose();
  viewStates.delete(projectUri(path).toString());
  void client.call("removeFile", { path }).catch(ignoreCrash);
  saveFiles(files);
  if (current === path) openFile(Object.keys(files)[0]);
  scheduleCheck();
}

async function start() {
  setStatus("Loading djls…");
  const opened = await client.open(files, SETTINGS);
  startupMs = opened.startupMs;
  await check();
}

async function recover(error: string) {
  setStatus(`djls crashed (${error}); restarting…`, true);
  client.restart();
  await start();
}

async function reset() {
  if (!confirm("Discard your changes and restore the example project?")) return;
  clearSavedFiles();
  for (const model of monaco.editor.getModels()) model.dispose();
  viewStates.clear();
  files = { ...EXAMPLE_FILES };
  diagnostics = {};
  client.restart();
  openFile(Object.keys(files).find((path) => path.endsWith("post_list.html"))!);
  await start();
}

document.getElementById("new-file")!.onclick = () => {
  const directory = current?.includes("/") ? current.slice(0, current.lastIndexOf("/") + 1) : "";
  const path = prompt("New file path (relative to the project root)", directory)?.trim().replace(/^\/+/, "");
  if (!path || path.endsWith("/")) return;
  if (path in files) {
    openFile(path);
    return;
  }
  addFile(path);
};
document.getElementById("reset")!.onclick = () => void reset();

openFile(current!);
renderProblems();
start().catch((error) => setStatus(`Failed to start djls: ${error}`, true));
