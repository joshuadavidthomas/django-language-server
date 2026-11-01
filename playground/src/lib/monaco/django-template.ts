import type { languages } from 'monaco-editor/editor';

export const DJANGO_TEMPLATE = 'django-html';

// Django syntax is matched before HTML in every state, so tags and variables
// highlight inside markup, attribute values, and `<script>` alike.
const django: languages.IMonarchLanguageRule[] = [
	[/\{#/, 'comment.django', '@lineComment'],
	[
		/(\{%-?)(\s*)(comment)\b/,
		['delimiter.django', '', { token: 'comment.django', next: '@blockComment' }]
	],
	[
		/(\{%-?)(\s*)(verbatim)(\s*)(-?%\})/,
		['delimiter.django', '', 'keyword.django', '', { token: 'delimiter.django', next: '@verbatim' }]
	],
	[
		/(\{%-?)(\s*)([A-Za-z_]\w*)/,
		['delimiter.django', '', { token: 'keyword.django', next: '@tag' }]
	],
	[/\{%-?/, 'delimiter.django', '@tag'],
	[/\{\{-?/, 'delimiter.django', '@variable']
];

// Shared by `{% %}` and `{{ }}` bodies.
const expression: languages.IMonarchLanguageRule[] = [
	[/"([^"\\]|\\.)*"/, 'string.django'],
	[/'([^'\\]|\\.)*'/, 'string.django'],
	[/\d+(\.\d+)?/, 'number.django'],
	[/(\|)(\s*)([A-Za-z_]\w*)/, ['operator.django', '', 'function.django']],
	[/(==|!=|<=|>=|<|>|=|:|,)/, 'operator.django'],
	[
		/\b(and|or|not|in|is|as|with|only|by|from|silent|reversed|asvar|noop|context|count|trimmed)\b/,
		'keyword.operator.django'
	],
	[/\b(True|False|None)\b/, 'constant.django'],
	[/[A-Za-z_][\w.]*/, 'variable.django'],
	[/\s+/, ''],
	[/./, '']
];

export const language: languages.IMonarchLanguage = {
	defaultToken: '',
	tokenPostfix: '.html',
	ignoreCase: true,
	tokenizer: {
		root: [
			...django,
			[/<!DOCTYPE/, 'metatag', '@doctype'],
			[/<!--/, 'comment', '@htmlComment'],
			[/(<)(\/?)([\w-]+)/, ['delimiter', 'delimiter', { token: 'tag', next: '@markup' }]],
			[/&\w+;/, 'string.escape'],
			[/[^<&{]+/, ''],
			[/./, '']
		],
		markup: [
			...django,
			[/\/?>/, 'delimiter', '@pop'],
			[/"/, 'attribute.value', '@doubleQuoted'],
			[/'/, 'attribute.value', '@singleQuoted'],
			[/[\w\-:@.]+/, 'attribute.name'],
			[/=/, 'delimiter'],
			[/\s+/, '']
		],
		doubleQuoted: [
			...django,
			[/[^"{]+/, 'attribute.value'],
			[/"/, 'attribute.value', '@pop'],
			[/\{/, 'attribute.value']
		],
		singleQuoted: [
			...django,
			[/[^'{]+/, 'attribute.value'],
			[/'/, 'attribute.value', '@pop'],
			[/\{/, 'attribute.value']
		],
		doctype: [
			[/[^>]+/, 'metatag.content'],
			[/>/, 'metatag', '@pop']
		],
		htmlComment: [
			[/-->/, 'comment', '@pop'],
			[/[^-]+/, 'comment'],
			[/./, 'comment']
		],
		tag: [[/-?%\}/, 'delimiter.django', '@pop'], ...expression],
		variable: [[/-?\}\}/, 'delimiter.django', '@pop'], ...expression],
		lineComment: [
			[/#\}/, 'comment.django', '@pop'],
			[/[^#]+/, 'comment.django'],
			[/#/, 'comment.django']
		],
		blockComment: [
			[/\{%-?\s*endcomment\s*-?%\}/, 'comment.django', '@pop'],
			[/[^{]+/, 'comment.django'],
			[/\{/, 'comment.django']
		],
		verbatim: [
			[
				/(\{%-?)(\s*)(endverbatim)(\s*)(-?%\})/,
				['delimiter.django', '', 'keyword.django', '', { token: 'delimiter.django', next: '@pop' }]
			],
			[/[^{]+/, ''],
			[/\{/, '']
		]
	}
};

export const configuration: languages.LanguageConfiguration = {
	comments: { blockComment: ['{#', '#}'] },
	brackets: [
		['{%', '%}'],
		['{{', '}}'],
		['<', '>']
	],
	autoClosingPairs: [
		// Django delimiters close as a pair: typing `{%` gives `{%| %}`.
		{ open: '{%', close: ' %}' },
		{ open: '{{', close: ' }}' },
		{ open: '{#', close: ' #}' },
		{ open: '"', close: '"', notIn: ['string'] },
		{ open: "'", close: "'", notIn: ['string'] },
		{ open: '<!--', close: '-->', notIn: ['comment', 'string'] }
	],
	surroundingPairs: [
		{ open: '"', close: '"' },
		{ open: "'", close: "'" },
		{ open: '{', close: '}' },
		{ open: '<', close: '>' }
	],
	wordPattern: /[A-Za-z_][\w.-]*/
};
