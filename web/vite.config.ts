import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vitest/config';
import { svelteTesting } from '@testing-library/svelte/vite';
import { codecovSvelteKitPlugin } from '@codecov/sveltekit-plugin';
import { notices } from './notices';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';

export default defineConfig({
	plugins: [
		tailwindcss(),
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			// Single-page app: every unknown path gets 200.html, which the Rust server
			// embeds and serves as the fallback (see crates/lodger/src/assets.rs later).
			adapter: adapter({ fallback: '200.html' })
		}),
		// License check and /third-party-notices.txt (notices.ts).
		...notices(import.meta.dirname),
		// SvelteKit names its output `__sveltekit_<build hash>.app`. ES modules
		// ignore that name, but the Codecov plugin appends it to the bundle
		// name, so each build would start a new bundle in Bundle Analysis.
		// Removing it from ES output changes nothing in the build. SvelteKit's
		// inline (iife) mode needs the name, so that mode keeps it.
		{
			name: 'lodger-stable-bundle-name',
			outputOptions(options) {
				if (options.format === 'es' || options.format === 'esm') {
					return { ...options, name: undefined };
				}
				return null;
			}
		},
		// Codecov Bundle Analysis. It must come after every other plugin. It
		// uploads bundle sizes during `pnpm build`, and only on GitHub Actions,
		// so a local build sends nothing. The repository is public, so GitHub
		// Actions needs no upload token (Codecov's tokenless upload).
		codecovSvelteKitPlugin({
			enableBundleAnalysis: process.env.GITHUB_ACTIONS === 'true',
			bundleName: 'lodger-web',
			gitService: 'github',
			telemetry: false
		})
	],
	// `pnpm dev` serves the UI with hot reload and sends API and WebSocket
	// requests to a local `lodger serve` on its default port.
	server: {
		proxy: {
			'/api': 'http://127.0.0.1:8460',
			'/ws': { target: 'ws://127.0.0.1:8460', ws: true }
		}
	},
	test: {
		expect: { requireAssertions: true },
		// lcov for Codecov's `ui` flag (see .github/workflows/ci.yml and codecov.yml).
		coverage: {
			provider: 'v8',
			reporter: ['text', 'lcov'],
			include: ['src/**/*.{ts,svelte}'],
			// shadcn-svelte code copied from upstream, and test helpers. The same
			// paths are in codecov.yml `ignore:`.
			exclude: ['src/lib/components/ui/**', 'src/lib/test/**']
		},
		projects: [
			{
				// Components, rendered in jsdom with Testing Library.
				extends: './vite.config.ts',
				plugins: [svelteTesting()],
				test: {
					name: 'client',
					environment: 'jsdom',
					include: ['src/**/*.svelte.{test,spec}.{js,ts}'],
					setupFiles: ['./src/lib/test/setup.ts']
				}
			},
			{
				extends: './vite.config.ts',
				test: {
					name: 'server',
					environment: 'node',
					include: ['src/**/*.{test,spec}.{js,ts}'],
					exclude: ['src/**/*.svelte.{test,spec}.{js,ts}']
				}
			}
		]
	}
});
