// The subset of LSP types `djls-wasm` returns. Positions are zero-based UTF-16.

export interface Position {
	line: number;
	character: number;
}

export interface Range {
	start: Position;
	end: Position;
}

export interface Diagnostic {
	range: Range;
	severity?: 1 | 2 | 3 | 4;
	code?: string | number;
	source?: string;
	message: string;
	tags?: (1 | 2)[];
}

export interface MarkupContent {
	kind: 'plaintext' | 'markdown';
	value: string;
}

export interface TextEdit {
	range: Range;
	newText: string;
}

export interface InsertReplaceEdit {
	newText: string;
	insert: Range;
	replace: Range;
}

export interface CompletionItem {
	label: string;
	kind?: number;
	detail?: string;
	/** djls sends plain text here. */
	documentation?: string;
	sortText?: string;
	filterText?: string;
	insertText?: string;
	insertTextFormat?: 1 | 2;
	textEdit?: TextEdit | InsertReplaceEdit;
	additionalTextEdits?: TextEdit[];
	preselect?: boolean;
	deprecated?: boolean;
	tags?: 1[];
}

export interface Hover {
	/** djls sends markup here, never the deprecated `MarkedString` forms. */
	contents: MarkupContent;
	range?: Range;
}

export interface LocationLink {
	originSelectionRange?: Range;
	targetUri: string;
	targetRange: Range;
	targetSelectionRange: Range;
}

export interface FoldingRange {
	startLine: number;
	startCharacter?: number;
	endLine: number;
	endCharacter?: number;
	kind?: string;
}
