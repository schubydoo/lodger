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

import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import license, { type Dependency } from 'rollup-plugin-license';
import satisfies from 'spdx-satisfies';
import validExpression from 'spdx-expression-validate';
import type { Plugin } from 'vite';

const ALLOWED = [
	'MIT',
	'ISC',
	'Apache-2.0',
	'BSD-2-Clause',
	'BSD-3-Clause',
	'0BSD',
	'MPL-2.0',
	'CC0-1.0'
];

/**
 * Packages whose package.json names no license, with the license that their
 * LICENSE file states. Each entry names one version and the start of the
 * license text, so an update or a changed text fails the build again.
 */
const LICENSE_FIXES: Record<string, { license: string; textStart: string }> = {
	// A bits-ui dependency. 0.10.6 is the latest release, and its LICENSE
	// file is MIT (Hunter Johnston and Thomas G. Lopes).
	'svelte-toolbelt@0.10.6': { license: 'MIT', textStart: 'MIT License' }
};

/** The SPDX license of a package, from package.json or from LICENSE_FIXES. */
export function licenseOf(dep: Dependency): string | null {
	if (dep.license) return dep.license;
	const fix = LICENSE_FIXES[`${dep.name}@${dep.version}`];
	return fix && dep.licenseText?.trim().startsWith(fix.textStart) ? fix.license : null;
}

/** Whether a package may go into the bundle: its license is in ALLOWED. */
export function allowed(dep: Dependency): boolean {
	const spdx = licenseOf(dep);
	return spdx !== null && validExpression(spdx) && satisfies(spdx, ALLOWED);
}

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
	const rustFile = `${root}/notices/rust.txt`;
	const rust = () => (existsSync(rustFile) ? readFileSync(rustFile, 'utf8') : null);
	const copied = () => readFileSync(`${root}/notices/copied.txt`, 'utf8');
	return [
		license({
			thirdParty: {
				allow: { test: allowed, failOnUnlicensed: true, failOnViolation: true },
				output: (deps) => {
					packages = deps.map((dep) => ({ ...dep, license: licenseOf(dep) }));
				}
			}
		}) as Plugin,
		{
			name: 'lodger-third-party-notices',
			// `pnpm run dev` builds no bundle, so the dev server answers the
			// footer link itself. The package list exists only in a build.
			configureServer(server) {
				server.middlewares.use('/third-party-notices.txt', (_req, res) => {
					res.setHeader('content-type', 'text/plain; charset=utf-8');
					res.end(
						joinNotices(
							copied(),
							'Web UI packages\n===============\n\nListed only in a build (`pnpm run build`).\n',
							rust()
						)
					);
				});
			},
			// Written to disk after the bundle, not emitted into it: Codecov
			// Bundle Analysis reads the bundle, and a 240 kB text file that the
			// browser loads only on request is not part of the app's size.
			writeBundle(options) {
				// SvelteKit builds the server part too; the notices belong to
				// the client output, which the binary serves.
				if (this.environment?.name !== 'client' || !options.dir) return;
				const rustPart = rust();
				if (rustPart === null) {
					this.warn(
						'web/notices/rust.txt is missing, so /third-party-notices.txt has no Rust crates. Run `just notices` first.'
					);
				}
				writeFileSync(
					`${options.dir}/third-party-notices.txt`,
					joinNotices(copied(), formatPackages(packages), rustPart)
				);
			}
		}
	];
}
