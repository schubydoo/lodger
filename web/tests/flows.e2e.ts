// The main flows (Task 3.11): setup, login, the VM list, lifecycle actions,
// and pool creation, on libvirt's test driver. Each flow uses the keyboard
// alone, and axe checks every page that it visits.
import { expect, test, type Page } from '@playwright/test';
import { admin, expectAccessible, setupToken, tabTo, typeInto } from './helpers';

test.describe.configure({ mode: 'serial' });

let page: Page;

test.beforeAll(async ({ browser }) => {
	// axe needs a page from an explicit context.
	const context = await browser.newContext();
	page = await context.newPage();
});

test.afterAll(async () => {
	await page.context().close();
});

test('first-run setup creates the account', async () => {
	await page.goto('/');
	await expect(page).toHaveURL(/\/setup$/);
	await expectAccessible(page);
	await typeInto(page, page.getByLabel('Setup token'), setupToken());
	await typeInto(page, page.getByLabel('Username'), admin.username);
	await typeInto(page, page.getByLabel('Password', { exact: true }), admin.password);
	await typeInto(page, page.getByLabel(/again/i), admin.password);
	await tabTo(page, page.getByRole('button', { name: 'Create the account' }));
	await page.keyboard.press('Enter');
	await expect(page).not.toHaveURL(/\/setup$/);
});

test('login opens the host overview', async () => {
	// A fresh page has no session, whatever setup did.
	await page.context().clearCookies();
	await page.goto('/login');
	await expectAccessible(page);
	await typeInto(page, page.getByLabel('Username'), admin.username);
	await typeInto(page, page.getByLabel('Password'), admin.password);
	await page.keyboard.press('Enter');
	await expect(page).toHaveURL(/\/$/);
	await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
	await expectAccessible(page);
});

test('the VM list shows the test VM, and pause and resume work', async () => {
	await page.goto('/vms');
	const row = page.getByRole('row').filter({ hasText: 'test' });
	await expect(row).toContainText('Running');
	await expectAccessible(page);
	await tabTo(page, row.getByRole('button', { name: 'Pause' }));
	await page.keyboard.press('Enter');
	await expect(row).toContainText('Paused');
	await tabTo(page, row.getByRole('button', { name: 'Resume' }));
	await page.keyboard.press('Enter');
	await expect(row).toContainText('Running');
	await expectAccessible(page);
});

test('a VM page opens from the list', async () => {
	await page.goto('/vms');
	await tabTo(page, page.getByRole('link', { name: 'test', exact: true }));
	await page.keyboard.press('Enter');
	await expect(page).toHaveURL(/\/vms\/test$/);
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('test');
	await expectAccessible(page);
});

test('a folder pool is created and opens its page', async () => {
	await page.goto('/storage');
	await expectAccessible(page);
	await typeInto(page, page.getByLabel('Name'), 'e2e-pool');
	await typeInto(page, page.getByRole('textbox', { name: 'Folder' }), '/srv/e2e-pool');
	await tabTo(page, page.getByRole('button', { name: 'Create pool' }));
	await page.keyboard.press('Enter');
	await expect(page).toHaveURL(/\/storage\/e2e-pool$/);
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('e2e-pool');
	await expect(page.getByText('running', { exact: true })).toBeVisible();
	await expectAccessible(page);
});

test('the networks and account pages pass axe', async () => {
	await page.goto('/networks');
	await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
	await expectAccessible(page);
	await page.goto('/account');
	await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
	await expectAccessible(page);
});
