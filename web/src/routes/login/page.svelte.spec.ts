import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import Page from './+page.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { testClient } from '$lib/test/fixtures';
import { keys } from '$lib/api';

function answer(body: unknown, status: number) {
	return new Response(JSON.stringify(body), { status });
}

async function logIn(fetcher: typeof fetch) {
	vi.stubGlobal('fetch', fetcher);
	const client = testClient();
	render(QueryHarness, { props: { client, component: Page, props: {} } });
	await fireEvent.input(screen.getByLabelText('Username'), { target: { value: 'admin' } });
	await fireEvent.input(screen.getByLabelText('Password'), { target: { value: 'the passphrase' } });
	await fireEvent.click(screen.getByRole('button', { name: 'Log in' }));
	return client;
}

afterEach(() => vi.unstubAllGlobals());

describe('the login page', () => {
	it('stores the session, which lets the app shell open the app', async () => {
		const fetcher = vi.fn(async () => answer({ username: 'admin', csrf_token: 'c' }, 200));
		const client = await logIn(fetcher);
		await vi.waitFor(() =>
			expect(client.getQueryData(keys.session)).toEqual({ username: 'admin', csrf_token: 'c' })
		);
		expect(fetcher).toHaveBeenCalledWith('/api/session', {
			method: 'POST',
			headers: { accept: 'application/json', 'content-type': 'application/json' },
			body: JSON.stringify({ username: 'admin', password: 'the passphrase' })
		});
	});

	it('says only that the name or the password is wrong', async () => {
		const client = await logIn(
			vi.fn(async () => answer({ error: 'wrong username or password' }, 401))
		);
		expect(await screen.findByRole('alert')).toHaveTextContent('Wrong username or password.');
		expect(client.getQueryData(keys.session)).toBeUndefined();
	});

	it('shows the throttle answer as a sentence', async () => {
		await logIn(
			vi.fn(async () => answer({ error: 'too many password attempts. Try again later' }, 429))
		);
		expect(await screen.findByRole('alert')).toHaveTextContent(
			'Too many password attempts. Try again later.'
		);
	});
});
