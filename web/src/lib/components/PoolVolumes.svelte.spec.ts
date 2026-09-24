import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import PoolVolumes from './PoolVolumes.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { testClient } from '$lib/test/fixtures';
import { keys, type PoolDetail, type Volume } from '$lib/api';

const pool: PoolDetail = {
	uuid: '00000000-0000-0000-0000-00000000000a',
	name: 'images',
	state: 'running',
	capacity_bytes: 1024 ** 4,
	allocation_bytes: 1024 ** 3,
	available_bytes: 1024 ** 4 - 1024 ** 3,
	persistent: true,
	autostart: true,
	kind: 'dir',
	path: '/var/lib/libvirt/images',
	used_by: []
};

const volume = (name: string, usedBy: string[] = [], backingFor: string[] = []): Volume => ({
	name,
	key: `/var/lib/libvirt/images/${name}`,
	path: `/var/lib/libvirt/images/${name}`,
	kind: 'file',
	format: 'qcow2',
	capacity_bytes: 20 * 1024 ** 3,
	allocation_bytes: 196 * 1024,
	used_by: usedBy,
	backing_for: backingFor
});

const json = (data: unknown, status = 200) =>
	new Response(JSON.stringify(data), { status, headers: { 'content-type': 'application/json' } });

/** Renders the section. `change` answers each POST and DELETE. */
function show(list: Volume[], change: () => Response = () => new Response(null, { status: 204 })) {
	const fetcher = vi.fn<typeof fetch>(async (_input, init) =>
		init?.method ? change() : json(list)
	);
	vi.stubGlobal('fetch', fetcher);
	const client = testClient();
	client.setQueryData(keys.session, { username: 'admin', csrf_token: 'csrf1' });
	render(QueryHarness, { props: { client, component: PoolVolumes, props: { pool } } });
	return { fetcher, client };
}

const changes = (fetcher: ReturnType<typeof show>['fetcher']) =>
	fetcher.mock.calls.filter(([, init]) => init?.method);

afterEach(() => vi.unstubAllGlobals());

describe('the volumes of a pool', () => {
	it('lists each volume with its format, sizes, and users', async () => {
		show([
			volume('base.qcow2', [], ['web.qcow2']),
			volume('disk1.qcow2'),
			volume('root.qcow2', ['db', 'web'])
		]);
		const region = await screen.findByRole('region', { name: 'Volumes of images' });
		const table = within(region).getByRole('table');
		const rows = within(table).getAllByRole('row');
		expect(rows[1]).toHaveTextContent('volume web.qcow2');
		expect(rows[2]).toHaveTextContent('disk1.qcow2');
		expect(rows[2]).toHaveTextContent('qcow2');
		expect(rows[2]).toHaveTextContent('20 GiB');
		expect(rows[3]).toHaveTextContent('db, web');
		expect(rows[3]).toHaveTextContent('In use');
		// A volume that a VM or an overlay uses has no delete button.
		expect(screen.queryByRole('button', { name: 'Delete root.qcow2' })).toBeNull();
		expect(screen.queryByRole('button', { name: 'Delete base.qcow2' })).toBeNull();
		expect(screen.getByRole('button', { name: 'Delete disk1.qcow2' })).toBeEnabled();
	});

	it('says so when the pool has no volumes', async () => {
		show([]);
		expect(await screen.findByText('images has no volumes.')).toBeInTheDocument();
	});

	it('creates a volume with its size in bytes and refreshes the pool', async () => {
		const { fetcher, client } = show([], () => json({ name: 'data.img' }, 201));
		const refresh = vi.spyOn(client, 'invalidateQueries');
		await screen.findByText('images has no volumes.');
		const create = screen.getByRole('button', { name: 'Create volume' });
		expect(create).toBeDisabled();
		await fireEvent.input(screen.getByLabelText('Name'), { target: { value: ' data.img ' } });
		await fireEvent.click(screen.getByLabelText('raw'));
		await fireEvent.input(screen.getByLabelText('Size (GiB)'), { target: { value: '1.5' } });
		await fireEvent.click(create);
		await waitFor(() => expect(changes(fetcher)).toHaveLength(1));
		const [url, init] = changes(fetcher)[0];
		expect(url).toBe(`/api/pools/${pool.uuid}/volumes`);
		expect(init?.method).toBe('POST');
		expect((init?.headers as Record<string, string>)['x-csrf-token']).toBe('csrf1');
		expect(JSON.parse(init?.body as string)).toEqual({
			name: 'data.img',
			format: 'raw',
			capacity_bytes: 1.5 * 1024 ** 3
		});
		await waitFor(() => expect(refresh).toHaveBeenCalledWith({ queryKey: keys.pool(pool.uuid) }));
		expect(screen.getByLabelText('Name')).toHaveValue('');
	});

	it('deletes only after the exact name is typed', async () => {
		const { fetcher } = show([volume('disk 1.img')]);
		await fireEvent.click(await screen.findByRole('button', { name: 'Delete disk 1.img' }));
		const remove = screen.getByRole('button', { name: 'Delete' });
		const typeName = (value: string) =>
			fireEvent.input(screen.getByLabelText('Type disk 1.img to delete it'), {
				target: { value }
			});
		await typeName('disk 1');
		expect(remove).toBeDisabled();
		await typeName('disk 1.img');
		await fireEvent.click(remove);
		await waitFor(() => expect(changes(fetcher)).toHaveLength(1));
		const [url, init] = changes(fetcher)[0];
		expect(url).toBe(`/api/pools/${pool.uuid}/volumes/disk%201.img`);
		expect(init?.method).toBe('DELETE');
	});

	it('refetches the list on each visit, even with a cached one', async () => {
		const fetcher = vi.fn<typeof fetch>(async () => json([volume('fresh.img')]));
		vi.stubGlobal('fetch', fetcher);
		const client = testClient();
		client.setQueryData(keys.volumes(pool.uuid), [volume('stale.img')]);
		render(QueryHarness, { props: { client, component: PoolVolumes, props: { pool } } });
		expect(await screen.findByText('fresh.img')).toBeInTheDocument();
		expect(fetcher.mock.calls[0][0]).toBe(`/api/pools/${pool.uuid}/volumes`);
	});

	it('shows the server error, for example a VM that uses the volume', async () => {
		const { client } = show([volume('disk1.qcow2')], () => json({ error: 'in use by web' }, 409));
		const refresh = vi.spyOn(client, 'invalidateQueries');
		await fireEvent.click(await screen.findByRole('button', { name: 'Delete disk1.qcow2' }));
		await fireEvent.input(screen.getByLabelText('Type disk1.qcow2 to delete it'), {
			target: { value: 'disk1.qcow2' }
		});
		await fireEvent.click(screen.getByRole('button', { name: 'Delete' }));
		expect(await screen.findByText('In use by web.')).toBeInTheDocument();
		// The table catches up with the error.
		await waitFor(() => expect(refresh).toHaveBeenCalledWith({ queryKey: keys.pool(pool.uuid) }));
	});
});
