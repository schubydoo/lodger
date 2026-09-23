import { describe, expect, it } from 'vitest';
import type { Dependency } from 'rollup-plugin-license';
import { allowed, licenseOf } from './notices';

const dep = (fields: Partial<Dependency>) =>
	({ name: 'pkg', version: '1.0.0', licenseText: 'MIT License\n...', ...fields }) as Dependency;

describe('the license check', () => {
	it('allows the permissive licenses, also in expressions', () => {
		for (const license of ['MIT', 'Apache-2.0', '(MIT OR Apache-2.0)', 'MPL-2.0']) {
			expect(allowed(dep({ license })), license).toBe(true);
		}
	});

	it('refuses other licenses, bad expressions, and no license', () => {
		for (const license of ['GPL-3.0-only', 'AGPL-3.0-or-later', 'not a license', 'UNLICENSED']) {
			expect(allowed(dep({ license })), license).toBe(false);
		}
		expect(allowed(dep({ license: undefined }))).toBe(false);
	});

	it('fixes only the listed version, and only with the expected license text', () => {
		const toolbelt = { name: 'svelte-toolbelt', license: undefined };
		expect(licenseOf(dep({ ...toolbelt, version: '0.10.6' }))).toBe('MIT');
		expect(allowed(dep({ ...toolbelt, version: '0.10.6' }))).toBe(true);
		expect(allowed(dep({ ...toolbelt, version: '0.10.7' }))).toBe(false);
		expect(
			allowed(dep({ ...toolbelt, version: '0.10.6', licenseText: 'GNU GENERAL PUBLIC LICENSE' }))
		).toBe(false);
		// A package.json license wins over the fix.
		expect(licenseOf(dep({ ...toolbelt, version: '0.10.6', license: 'ISC' }))).toBe('ISC');
	});
});
