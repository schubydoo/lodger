import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import { createRawSnippet } from 'svelte';
import AppShell from './AppShell.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { host, testClient } from '$lib/test/fixtures';
import { keys } from '$lib/api';

vi.mock('$lib/session', async (actual) => ({
	...(await actual<typeof import('$lib/session')>()),
	nextTicket: vi.fn(async () => 'tk')
}));
vi.mock('$app/navigation', () => ({ goto: vi.fn(async () => {}) }));
vi.mock('$app/state', () => ({ page: { url: new URL('http://lodger.test/') } }));

const children = createRawSnippet(() => ({ render: () => '<p>page body</p>' }));

afterEach(() => vi.unstubAllGlobals());

describe('the app shell', () => {
	it('drops the cached answers when the session ends without a logout', async () => {
		vi.stubGlobal(
			'fetch',
			vi.fn(async () => new Response('', { status: 404 }))
		);
		vi.stubGlobal(
			'WebSocket',
			class {
				close() {}
			}
		);
		const client = testClient();
		client.setQueryData(keys.session, { username: 'admin', csrf_token: 'csrf1' });
		client.setQueryData(keys.setup, false);
		client.setQueryData(keys.host, host);
		render(QueryHarness, { props: { client, component: AppShell, props: { children } } });
		expect(await screen.findByText('page body')).toBeInTheDocument();
		expect(client.getQueryData(keys.host)).toEqual(host);

		// What the layout does after a 401 from an expired session.
		client.setQueryData(keys.session, null);
		await vi.waitFor(() => expect(client.getQueryData(keys.host)).toBeUndefined());
		expect(client.getQueryData(keys.session)).toBeNull();
	});
});
