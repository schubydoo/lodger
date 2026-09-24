import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import PoolCreate from './PoolCreate.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { testClient } from '$lib/test/fixtures';
import { keys } from '$lib/api';

const nav = vi.hoisted(() => ({ goto: vi.fn(async () => {}) }));
vi.mock('$app/navigation', () => ({ goto: nav.goto }));

function show(
	respond: () => Response = () => new Response(JSON.stringify({ uuid: 'u1' }), { status: 201 })
) {
	const fetcher = vi.fn<typeof fetch>(async () => respond());
	vi.stubGlobal('fetch', fetcher);
	const client = testClient();
	client.setQueryData(keys.session, { username: 'admin', csrf_token: 'csrf1' });
	render(QueryHarness, { props: { client, component: PoolCreate, props: {} } });
	return { fetcher, client };
}

const type = (label: string, value: string) =>
	fireEvent.input(screen.getByLabelText(label), { target: { value } });
const sent = (fetcher: ReturnType<typeof show>['fetcher']) =>
	JSON.parse(fetcher.mock.calls[0][1]?.body as string);

beforeEach(() => nav.goto.mockClear());
afterEach(() => vi.unstubAllGlobals());

describe('the New pool form', () => {
	it('creates a folder pool with autostart and opens its page', async () => {
		const { fetcher, client } = show();
		const refresh = vi.spyOn(client, 'invalidateQueries');
		const create = screen.getByRole('button', { name: 'Create pool' });
		expect(create).toBeDisabled();
		await type('Name', 'images2');
		expect(create).toBeDisabled();
		await type('Folder', '/srv/images2');
		expect(create).toBeEnabled();
		await fireEvent.click(create);
		await waitFor(() => expect(nav.goto).toHaveBeenCalledWith('/storage/images2'));
		// The list is fresh before the new page reads it.
		expect(refresh).toHaveBeenCalledWith({ queryKey: keys.pools });
		expect(refresh.mock.invocationCallOrder[0]).toBeLessThan(nav.goto.mock.invocationCallOrder[0]);
		const [path, init] = fetcher.mock.calls[0];
		expect(path).toBe('/api/pools');
		expect(init?.method).toBe('POST');
		expect((init?.headers as Record<string, string>)['x-csrf-token']).toBe('csrf1');
		expect(sent(fetcher)).toEqual({
			kind: 'dir',
			name: 'images2',
			path: '/srv/images2',
			autostart: true
		});
	});

	it('creates an NFS pool without a folder, and with autostart off', async () => {
		const { fetcher } = show();
		await fireEvent.click(screen.getByLabelText('NFS share'));
		await type('Name', 'nas');
		await type('NFS server', 'nas.lan');
		const create = screen.getByRole('button', { name: 'Create pool' });
		expect(create).toBeDisabled();
		await type('Export path', '/volume1/vm');
		await fireEvent.click(screen.getByLabelText('Start the pool when the host boots'));
		await fireEvent.click(create);
		await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
		expect(sent(fetcher)).toEqual({
			kind: 'nfs',
			name: 'nas',
			host: 'nas.lan',
			export: '/volume1/vm',
			autostart: false
		});
	});

	it('sends the mount folder of an NFS pool when one is given', async () => {
		const { fetcher } = show();
		await fireEvent.click(screen.getByLabelText('NFS share'));
		await type('Name', 'nas');
		await type('NFS server', 'nas.lan');
		await type('Export path', '/volume1/vm');
		await type('Mount folder (optional)', '/mnt/nas');
		await fireEvent.click(screen.getByRole('button', { name: 'Create pool' }));
		await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
		expect(sent(fetcher).path).toBe('/mnt/nas');
	});

	it('shows the libvirt error of a failed mount and stays on the form', async () => {
		show(
			() =>
				new Response(
					JSON.stringify({ error: 'libvirt: internal error: Child process mount failed' }),
					{ status: 502 }
				)
		);
		await fireEvent.click(screen.getByLabelText('NFS share'));
		await type('Name', 'nas');
		await type('NFS server', 'nas.lan');
		await type('Export path', '/volume1/vm');
		await fireEvent.click(screen.getByRole('button', { name: 'Create pool' }));
		expect(await screen.findByRole('alert')).toHaveTextContent(
			'Libvirt: internal error: Child process mount failed.'
		);
		expect(nav.goto).not.toHaveBeenCalled();
	});
});
