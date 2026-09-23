import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/svelte';
import Page from './+page.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { testClient } from '$lib/test/fixtures';
import { keys, type Account } from '$lib/api';

const accounts: Account[] = [
	{
		id: 1,
		username: 'admin',
		created_at: '2026-09-22T10:00:00.000Z',
		password_changed_at: '2026-09-22T10:00:00.000Z',
		you: true
	},
	{
		id: 2,
		username: 'second',
		created_at: '2026-09-22T11:00:00.000Z',
		password_changed_at: '2026-09-22T11:00:00.000Z',
		you: false
	}
];

const COMMON = 'the password is on the list of the most common passwords. Choose another one';

function show(respond: (path: string, init?: RequestInit) => Response, list = accounts) {
	const fetcher = vi.fn(async (path: string, init?: RequestInit) => respond(path, init));
	vi.stubGlobal('fetch', fetcher);
	const client = testClient();
	client.setQueryData(keys.session, { username: 'admin', csrf_token: 'csrf1' });
	client.setQueryData(keys.accounts, list);
	render(QueryHarness, { props: { client, component: Page, props: {} } });
	return { client, fetcher };
}

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
const type = (label: string, value: string) =>
	fireEvent.input(screen.getByLabelText(label), { target: { value } });

afterEach(() => vi.unstubAllGlobals());

describe('the account page', () => {
	it('lists the accounts and marks the own one', () => {
		show(() => json(accounts));
		const rows = screen.getAllByRole('row');
		expect(rows[1]).toHaveTextContent('admin (you)');
		expect(rows[2]).toHaveTextContent('second');
		expect(rows[2]).not.toHaveTextContent('(you)');
	});

	it('changes the password with the current one and says how many sessions ended', async () => {
		const { fetcher } = show(() => json({ ended_sessions: 2 }));
		await type('Current password', 'the old passphrase');
		await type('New password', 'a new long passphrase');
		await type('New password again', 'a new long passphrase');
		await fireEvent.click(screen.getByRole('button', { name: 'Change the password' }));
		expect(await screen.findByRole('status')).toHaveTextContent(
			'Password changed. 2 other sessions ended.'
		);
		const [path, init] = fetcher.mock.calls[0];
		expect(path).toBe('/api/account/password');
		expect((init?.headers as Record<string, string>)['x-csrf-token']).toBe('csrf1');
		expect(JSON.parse(String(init?.body))).toEqual({
			current_password: 'the old passphrase',
			new_password: 'a new long passphrase'
		});
	});

	it('shows a clear message for a common password', async () => {
		show(() => json({ error: COMMON }, 422));
		await type('Current password', 'the old passphrase');
		await type('New password', 'demon1q2w3e4r5t');
		await type('New password again', 'demon1q2w3e4r5t');
		await fireEvent.click(screen.getByRole('button', { name: 'Change the password' }));
		expect(await screen.findByRole('alert')).toHaveTextContent(
			`${COMMON[0].toUpperCase()}${COMMON.slice(1)}.`
		);
	});

	it('refuses two different new passwords before it asks the server', async () => {
		const { fetcher } = show(() => json({}));
		await type('Current password', 'the old passphrase');
		await type('New password', 'a new long passphrase');
		await type('New password again', 'a new long passphrase!');
		await fireEvent.click(screen.getByRole('button', { name: 'Change the password' }));
		expect(await screen.findByRole('alert')).toHaveTextContent('not the same');
		expect(fetcher).not.toHaveBeenCalled();
	});

	it('adds an account and loads the list again', async () => {
		const { fetcher } = show((path, init) =>
			init?.method === 'POST' ? json({ id: 3, username: 'third' }, 201) : json(accounts)
		);
		await type('Username', 'third');
		await type('Password', 'a third long passphrase');
		await fireEvent.click(screen.getByRole('button', { name: 'Add the account' }));
		await vi.waitFor(() =>
			expect(fetcher).toHaveBeenCalledWith('/api/accounts', expect.anything())
		);
		const [path, init] = fetcher.mock.calls[0];
		expect(path).toBe('/api/accounts');
		expect(JSON.parse(String(init?.body))).toEqual({
			username: 'third',
			password: 'a third long passphrase'
		});
	});

	it('asks before it deletes, and shows why the last account stays', async () => {
		const { fetcher } = show(
			() =>
				json(
					{ error: 'this is the last account. Add another account before you delete this one' },
					409
				),
			[accounts[0]]
		);
		await fireEvent.click(screen.getByRole('button', { name: 'Delete admin' }));
		expect(fetcher).not.toHaveBeenCalled();
		await fireEvent.click(screen.getByRole('button', { name: 'Yes, delete' }));
		expect(await screen.findByRole('alert')).toHaveTextContent(
			'This is the last account. Add another account before you delete this one.'
		);
		expect(fetcher.mock.calls[0][0]).toBe('/api/accounts/1');
		expect(screen.getByRole('button', { name: 'Delete admin' })).toBeInTheDocument();
	});

	it('keeps the account when the user changes their mind', async () => {
		const { fetcher } = show(() => json({}));
		const row = screen.getAllByRole('row')[2];
		await fireEvent.click(within(row).getByRole('button', { name: 'Delete second' }));
		await fireEvent.click(within(row).getByRole('button', { name: 'Keep' }));
		expect(fetcher).not.toHaveBeenCalled();
	});

	it('forgets the session after deleting the own account', async () => {
		const { client } = show(() => new Response(null, { status: 204 }));
		const row = screen.getAllByRole('row')[1];
		await fireEvent.click(within(row).getByRole('button', { name: 'Delete admin' }));
		await fireEvent.click(within(row).getByRole('button', { name: 'Yes, delete' }));
		await vi.waitFor(() => expect(client.getQueryData(keys.session)).toBeNull());
	});

	it('forgets the session when the server answers 401', async () => {
		const { client } = show(() => json({ error: 'log in first' }, 401));
		await type('Username', 'third');
		await type('Password', 'a third long passphrase');
		await fireEvent.click(screen.getByRole('button', { name: 'Add the account' }));
		await vi.waitFor(() => expect(client.getQueryData(keys.session)).toBeNull());
	});
});
