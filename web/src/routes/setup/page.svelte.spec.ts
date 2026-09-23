import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import Page from './+page.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { testClient } from '$lib/test/fixtures';
import { keys } from '$lib/api';

async function fill(password: string, confirm = password) {
	const client = testClient();
	render(QueryHarness, { props: { client, component: Page, props: {} } });
	const type = (label: string, value: string) =>
		fireEvent.input(screen.getByLabelText(label), { target: { value } });
	await type('Setup token', 'tok');
	await type('Username', 'admin');
	await type('Password', password);
	await type('Password again', confirm);
	await fireEvent.click(screen.getByRole('button', { name: 'Create the account' }));
	return client;
}

afterEach(() => vi.unstubAllGlobals());

describe('the setup page', () => {
	it('creates the account, then logs in', async () => {
		const fetcher = vi.fn(async (path: string) =>
			path === '/api/setup'
				? new Response(JSON.stringify({ username: 'admin' }), { status: 201 })
				: new Response(JSON.stringify({ username: 'admin', csrf_token: 'c' }), { status: 200 })
		);
		vi.stubGlobal('fetch', fetcher);
		const client = await fill('a good long passphrase');
		await vi.waitFor(() => expect(client.getQueryData(keys.session)).toBeTruthy());
		expect(client.getQueryData(keys.setup)).toBe(false);
		expect(fetcher.mock.calls.map(([p]) => p)).toEqual(['/api/setup', '/api/session']);
	});

	it('refuses two different passwords before it asks the server', async () => {
		const fetcher = vi.fn();
		vi.stubGlobal('fetch', fetcher);
		await fill('a good long passphrase', 'another long passphrase');
		expect(await screen.findByRole('alert')).toHaveTextContent(
			'The two passwords are not the same.'
		);
		expect(fetcher).not.toHaveBeenCalled();
	});

	it("shows the server's reason, for example a common password", async () => {
		vi.stubGlobal(
			'fetch',
			vi.fn(
				async () =>
					new Response(
						JSON.stringify({
							error: 'the password is on the list of the most common passwords. Choose another one'
						}),
						{ status: 422 }
					)
			)
		);
		await fill('demon1q2w3e4r5t');
		expect(await screen.findByRole('alert')).toHaveTextContent(
			'The password is on the list of the most common passwords. Choose another one.'
		);
	});
});
