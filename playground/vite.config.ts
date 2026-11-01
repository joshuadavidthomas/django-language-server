import tailwindcss from '@tailwindcss/vite';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [
		tailwindcss(),
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			// No server: djls runs in the browser, so the site is one prerendered shell.
			adapter: adapter()
		})
	],
	// The djls worker and the wasm-bindgen glue are ES modules.
	worker: { format: 'es' },
	// Prebundling splits Monaco's side-effect imports into separate copies, so contributions
	// such as Go to Definition miss the editor's registry.
	optimizeDeps: { exclude: ['monaco-editor'] },
	// Monaco alone is several megabytes; splitting it buys nothing here.
	build: { chunkSizeWarningLimit: 5000 },
	server: {
		// An orb portal's host is only known when it starts: accept any Host in dev.
		allowedHosts: true,
		// The wasm and Django zip that `just playground wasm` writes, outside SvelteKit's allow list.
		fs: { allow: ['djls-wasm'] }
	}
});
