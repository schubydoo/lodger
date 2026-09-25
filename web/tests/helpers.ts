import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import AxeBuilder from '@axe-core/playwright';
import { expect, type Locator, type Page } from '@playwright/test';

/** The test account. The password passes Lodger's policy. */
export const admin = { username: 'e2eadmin', password: 'correct horse battery staple e2e' };

/** The setup token that the server wrote to its log. */
export function setupToken(): string {
	const log = readFileSync(join(import.meta.dirname, '.state', 'server.log'), 'utf8');
	const match = /lodger: setup token: (\S+)/.exec(log);
	if (!match) throw new Error(`no setup token in the server log:\n${log}`);
	return match[1];
}

/**
 * Moves the focus with Tab until `target` has it, as a keyboard user would.
 * It fails if the target cannot be reached in `max` presses, which proves
 * that the flow works with the keyboard alone.
 */
export async function tabTo(page: Page, target: Locator, max = 60): Promise<void> {
	for (let i = 0; i < max; i++) {
		if (await target.evaluate((el) => el === document.activeElement)) return;
		await page.keyboard.press('Tab');
	}
	await expect(target, `not reachable with ${max} presses of Tab`).toBeFocused();
}

/** Types into a field that the keyboard reaches first. */
export async function typeInto(page: Page, field: Locator, text: string): Promise<void> {
	await tabTo(page, field);
	await page.keyboard.type(text);
}

/** Fails on any WCAG 2.2 A or AA violation that axe finds on the page. */
export async function expectAccessible(page: Page): Promise<void> {
	const result = await new AxeBuilder({ page })
		.withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'])
		.analyze();
	const summary = result.violations.map(
		(v) => `${v.id} (${v.impact}): ${v.help} - ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`
	);
	expect(summary, `axe on ${page.url()}`).toEqual([]);
}
