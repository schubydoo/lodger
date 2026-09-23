// Builds /third-party-notices.txt for the web UI and the binary (Task 1.11).
//
// The file has three parts:
// 1. notices/copied.txt: code that the repository copies (shadcn-svelte) and
//    code that a package carries inside it (pako in noVNC). Kept by hand.
// 2. The npm packages that the client bundle contains, from
//    rollup-plugin-license. Only modules that end up in the bundle count.
// 3. notices/rust.txt: the Rust crates in the binary, from cargo-about
//    (`just notices`). CI and release builds generate it; a local build
//    without it says so in the file.
//
// The license check fails the build when a bundled package has no license or
// one outside ALLOWED. Keep ALLOWED in step with about.toml and deny.toml.

import { existsSync, readFileSync } from 'node:fs';
import license, { type Dependency } from 'rollup-plugin-license';
import type { Plugin } from 'vite';

const ALLOWED =
	'(MIT OR ISC OR Apache-2.0 OR BSD-2-Clause OR BSD-3-Clause OR 0BSD OR MPL-2.0 OR CC0-1.0)';

const HEADER = `Lodger third-party notices
==========================

Lodger is licensed under the Apache License 2.0. The lodger binary and its
web UI include the third-party works below. Each notice is reproduced as its
license requires. The build generates this file; do not edit it by hand.
`;

function repositoryUrl(dep: Dependency): string {
	const repo = dep.repository;
	if (!repo) return '';
	return typeof repo === 'string' ? repo : (repo.url ?? '');
}

/** Formats the npm packages of the client bundle. */
export function formatPackages(deps: Dependency[]): string {
	const sorted = [...deps].sort((a, b) => (a.name ?? '').localeCompare(b.name ?? ''));
	const blocks = sorted.map((dep) => {
		const url = repositoryUrl(dep);
		const lines = [
			'-----------------------------------------------------------------------------',
			`${dep.name} ${dep.version} (${dep.license})${url ? ` - ${url}` : ''}`,
			'',
			(dep.licenseText ?? `No license file in the package. License: ${dep.license}.`).trim()
		];
		if (dep.noticeText) lines.push('', dep.noticeText.trim());
		return lines.join('\n');
	});
	return `Web UI packages\n===============\n\n${blocks.join('\n\n\n')}\n`;
}

/** Joins the parts into the final file. */
export function joinNotices(copied: string, packages: string, rust: string | null): string {
	const rustPart =
		rust ??
		'Rust crate notices\n==================\n\nThis build did not generate them. Run `just notices` before the web build;\nCI and release builds do.\n';
	return [HEADER, copied.trim() + '\n', packages, rustPart].join('\n\n');
}

/** The Vite plugins that check the licenses and emit the notices file. */
export function notices(root: string): Plugin[] {
	let packages: Dependency[] = [];
	return [
		license({
			thirdParty: {
				allow: { test: ALLOWED, failOnUnlicensed: true, failOnViolation: true },
				output: (deps) => {
					packages = deps;
				}
			}
		}) as Plugin,
		{
			name: 'lodger-third-party-notices',
			generateBundle() {
				// SvelteKit builds the server part too; the notices belong to
				// the client output, which the binary serves.
				if (this.environment?.name !== 'client') return;
				const rustFile = `${root}/notices/rust.txt`;
				this.emitFile({
					type: 'asset',
					fileName: 'third-party-notices.txt',
					source: joinNotices(
						readFileSync(`${root}/notices/copied.txt`, 'utf8'),
						formatPackages(packages),
						existsSync(rustFile) ? readFileSync(rustFile, 'utf8') : null
					)
				});
			}
		}
	];
}
