// Monaco's one-time global setup: the features the playground uses, the editor worker, the
// Django template language, and themes that sit on the page's background.
import 'monaco-editor/features/register.all';
// `register.all` leaves out the Go to Definition / Peek commands that `editor.main` adds.
import 'monaco-editor/editor/contrib/gotoSymbol/browser/goToCommands';
import 'monaco-editor/languages/definitions/python/register';
import * as monaco from 'monaco-editor/editor';
import EditorWorker from 'monaco-editor/editor/editor.worker?worker';
import * as djangoTemplate from '#lib/monaco/django-template.js';

self.MonacoEnvironment = { getWorker: () => new EditorWorker() };

monaco.languages.register({ id: djangoTemplate.DJANGO_TEMPLATE, aliases: ['Django HTML'] });
monaco.languages.setMonarchTokensProvider(djangoTemplate.DJANGO_TEMPLATE, djangoTemplate.language);
monaco.languages.setLanguageConfiguration(
	djangoTemplate.DJANGO_TEMPLATE,
	djangoTemplate.configuration
);

// The editor's background is the page's `--background` (layout.css), so it reads as part of it.
monaco.editor.defineTheme('djls-dark', {
	base: 'vs-dark',
	inherit: true,
	rules: [
		{ token: 'delimiter.django', foreground: 'e5a04a', fontStyle: 'bold' },
		{ token: 'keyword.django', foreground: 'c586c0' },
		{ token: 'keyword.operator.django', foreground: '569cd6' },
		{ token: 'function.django', foreground: 'dcdcaa' },
		{ token: 'variable.django', foreground: '9cdcfe' },
		{ token: 'string.django', foreground: 'ce9178' },
		{ token: 'number.django', foreground: 'b5cea8' },
		{ token: 'constant.django', foreground: '569cd6' },
		{ token: 'operator.django', foreground: 'd4d4d4' },
		{ token: 'comment.django', foreground: '6a9955', fontStyle: 'italic' }
	],
	colors: { 'editor.background': '#0a0a0a', 'editorGutter.background': '#0a0a0a' }
});

monaco.editor.defineTheme('djls-light', {
	base: 'vs',
	inherit: true,
	rules: [
		{ token: 'delimiter.django', foreground: 'b26a00', fontStyle: 'bold' },
		{ token: 'keyword.django', foreground: 'af00db' },
		{ token: 'keyword.operator.django', foreground: '0000ff' },
		{ token: 'function.django', foreground: '795e26' },
		{ token: 'variable.django', foreground: '001080' },
		{ token: 'string.django', foreground: 'a31515' },
		{ token: 'number.django', foreground: '098658' },
		{ token: 'constant.django', foreground: '0000ff' },
		{ token: 'operator.django', foreground: '000000' },
		{ token: 'comment.django', foreground: '008000', fontStyle: 'italic' }
	],
	colors: { 'editor.background': '#ffffff', 'editorGutter.background': '#ffffff' }
});

export { monaco };
