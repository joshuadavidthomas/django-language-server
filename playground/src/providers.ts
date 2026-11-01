import * as monaco from "monaco-editor/editor";
import type { DjlsClient } from "./client";
import { DJANGO_TEMPLATE } from "./django-template";
import type * as lsp from "./lsp";

// Project files live under `/project`, matching the `file://` URIs djls returns.
const PROJECT_ROOT = "/project/";

export function projectUri(path: string): monaco.Uri {
  return monaco.Uri.file(PROJECT_ROOT + path);
}

/** Project-relative path of a URI, or `null` for files outside the project such as bundled Django. */
export function projectPath(uri: monaco.Uri): string | null {
  return uri.path.startsWith(PROJECT_ROOT) ? uri.path.slice(PROJECT_ROOT.length) : null;
}

function filePosition(model: monaco.editor.ITextModel, position: monaco.Position) {
  const path = projectPath(model.uri);
  return path === null ? null : { path, line: position.lineNumber - 1, character: position.column - 1 };
}

function toRange(range: lsp.Range): monaco.IRange {
  return {
    startLineNumber: range.start.line + 1,
    startColumn: range.start.character + 1,
    endLineNumber: range.end.line + 1,
    endColumn: range.end.character + 1,
  };
}

const SEVERITIES: Record<number, monaco.MarkerSeverity> = {
  1: monaco.MarkerSeverity.Error,
  2: monaco.MarkerSeverity.Warning,
  3: monaco.MarkerSeverity.Info,
  4: monaco.MarkerSeverity.Hint,
};

export function toMarker(diagnostic: lsp.Diagnostic): monaco.editor.IMarkerData {
  return {
    ...toRange(diagnostic.range),
    severity: SEVERITIES[diagnostic.severity ?? 1],
    message: diagnostic.message,
    code: diagnostic.code === undefined ? undefined : String(diagnostic.code),
    source: diagnostic.source,
    tags: diagnostic.tags?.map((tag) => (tag === 1 ? monaco.MarkerTag.Unnecessary : monaco.MarkerTag.Deprecated)),
  };
}

// LSP `CompletionItemKind` is 1-based in this order; Monaco's enum uses the same names.
const LSP_COMPLETION_KINDS = [
  "Text", "Method", "Function", "Constructor", "Field", "Variable", "Class", "Interface", "Module",
  "Property", "Unit", "Value", "Enum", "Keyword", "Snippet", "Color", "File", "Reference", "Folder",
  "EnumMember", "Constant", "Struct", "Event", "Operator", "TypeParameter",
] as const;

function toCompletionKind(kind: number | undefined): monaco.languages.CompletionItemKind {
  const name = kind === undefined ? undefined : LSP_COMPLETION_KINDS[kind - 1];
  return monaco.languages.CompletionItemKind[name ?? "Text"];
}

function toMarkdown(content: string | lsp.MarkupContent | lsp.MarkedString): monaco.IMarkdownString {
  if (typeof content === "string") return { value: content };
  if ("kind" in content) {
    return { value: content.kind === "markdown" ? content.value : escapeMarkdown(content.value) };
  }
  return { value: `\`\`\`${content.language}\n${content.value}\n\`\`\`` };
}

function escapeMarkdown(text: string): string {
  return text.replace(/[\\`*_{}[\]()#+\-.!|<>]/g, "\\$&");
}

function toCompletion(item: lsp.CompletionItem, wordRange: monaco.IRange): monaco.languages.CompletionItem {
  const edit = item.textEdit;
  let range: monaco.languages.CompletionItem["range"] = wordRange;
  if (edit && "range" in edit) range = toRange(edit.range);
  if (edit && "insert" in edit) range = { insert: toRange(edit.insert), replace: toRange(edit.replace) };
  return {
    label: item.label,
    kind: toCompletionKind(item.kind),
    detail: item.detail,
    documentation: item.documentation === undefined ? undefined : toMarkdown(item.documentation),
    sortText: item.sortText,
    filterText: item.filterText,
    preselect: item.preselect,
    tags: item.deprecated || item.tags?.includes(1) ? [monaco.languages.CompletionItemTag.Deprecated] : undefined,
    insertText: edit?.newText ?? item.insertText ?? item.label,
    insertTextRules:
      item.insertTextFormat === 2 ? monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet : undefined,
    range,
    additionalTextEdits: item.additionalTextEdits?.map((textEdit) => ({
      range: toRange(textEdit.range),
      text: textEdit.newText,
    })),
  };
}

/**
 * Language features go through the worker. `ensureModel` must create a model
 * for a definition target before Monaco can peek at or open it.
 */
export function registerProviders(
  client: DjlsClient,
  ensureModel: (uri: monaco.Uri) => Promise<void>,
): monaco.IDisposable[] {
  return [
    monaco.languages.registerCompletionItemProvider(DJANGO_TEMPLATE, {
      // The same trigger characters djls-server advertises.
      triggerCharacters: ["{", "%", " ", '"', "'"],
      async provideCompletionItems(model, position) {
        const at = filePosition(model, position);
        if (!at) return { suggestions: [] };
        const items = await client.call("completions", at);
        const word = model.getWordUntilPosition(position);
        const wordRange = new monaco.Range(position.lineNumber, word.startColumn, position.lineNumber, word.endColumn);
        return { suggestions: items.map((item) => toCompletion(item, wordRange)) };
      },
    }),
    monaco.languages.registerHoverProvider(DJANGO_TEMPLATE, {
      async provideHover(model, position) {
        const at = filePosition(model, position);
        if (!at) return null;
        const hover = await client.call("hover", at);
        if (!hover) return null;
        const contents = Array.isArray(hover.contents) ? hover.contents : [hover.contents];
        return { contents: contents.map(toMarkdown), range: hover.range && toRange(hover.range) };
      },
    }),
    monaco.languages.registerDefinitionProvider(DJANGO_TEMPLATE, {
      async provideDefinition(model, position) {
        const at = filePosition(model, position);
        if (!at) return [];
        const links = await client.call("definition", at);
        return Promise.all(
          links.map(async (link) => {
            const uri = monaco.Uri.parse(link.targetUri);
            await ensureModel(uri);
            return {
              uri,
              range: toRange(link.targetRange),
              targetSelectionRange: toRange(link.targetSelectionRange),
              originSelectionRange: link.originSelectionRange && toRange(link.originSelectionRange),
            };
          }),
        );
      },
    }),
    monaco.languages.registerFoldingRangeProvider(DJANGO_TEMPLATE, {
      async provideFoldingRanges(model) {
        const path = projectPath(model.uri);
        if (path === null) return [];
        const ranges = await client.call("foldingRanges", { path });
        return ranges.map((range) => ({
          start: range.startLine + 1,
          end: range.endLine + 1,
          kind: range.kind === undefined ? undefined : monaco.languages.FoldingRangeKind.fromValue(range.kind),
        }));
      },
    }),
  ];
}
