import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import PoolManage from './PoolManage.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { testClient } from '$lib/test/fixtures';
import { keys, type PoolDetail } from '$lib/api';

const nav = vi.hoisted(() => ({ goto: vi.fn(async () => {}) }));
vi.mock('$app/navigation', () => ({ goto: nav.goto }));

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

function show(p: PoolDetail, respond: () => Response = () => new Response(null, { status: 204 })) {
	const fetcher = vi.fn<typeof fetch>(async () => respond());
	vi.stubGlobal('fetch', fetcher);
	const client = testClient();
	client.setQueryData(keys.session, { username: 'admin', csrf_token: 'csrf1' });
	render(QueryHarness, { props: { client, component: PoolManage, props: { pool: p } } });
	return { fetcher, client };
}

const button = (name: string) => screen.getByRole('button', { name });
const body = (fetcher: ReturnType<typeof show>['fetcher']) =>
	JSON.parse(fetcher.mock.calls[0][1]?.body as string);

beforeEach(() => nav.goto.mockClear());
afterEach(() => vi.unstubAllGlobals());

describe('a pool page', () => {
	it('stops a running pool and starts a stopped one', async () => {
		const { fetcher } = show(pool);
		await fireEvent.click(button('Stop'));
		await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
		expect(fetcher.mock.calls[0][0]).toBe(`/api/pools/${pool.uuid}`);
		expect(fetcher.mock.calls[0][1]?.method).toBe('PATCH');
		expect(body(fetcher)).toEqual({ active: false });
		const second = show({ ...pool, state: 'inactive' });
		await fireEvent.click(button('Start'));
		await waitFor(() => expect(second.fetcher).toHaveBeenCalledOnce());
		expect(body(second.fetcher)).toEqual({ active: true });
	});

	it('switches autostart', async () => {
		const { fetcher } = show(pool);
		await fireEvent.click(button('Turn autostart off'));
		await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
		expect(body(fetcher)).toEqual({ autostart: false });
	});

	it('lists the VMs that use the pool before a removal', () => {
		show({ ...pool, used_by: ['db', 'web'] });
		const note = screen.getByRole('note');
		expect(note).toHaveTextContent('These VMs have a disk in images.');
		expect(note).toHaveTextContent('db');
		expect(note).toHaveTextContent('web');
	});

	it('shows no list when no VM uses the pool', () => {
		show(pool);
		expect(screen.queryByRole('note')).toBeNull();
	});

	it('removes only after the exact name is typed, and keeps the files unless asked', async () => {
		const { fetcher, client } = show(pool);
		const refresh = vi.spyOn(client, 'invalidateQueries');
		await fireEvent.click(button('Remove images'));
		const remove = button('Remove');
		const typeName = (value: string) =>
			fireEvent.input(screen.getByLabelText('Type images to remove it'), { target: { value } });
		await typeName('IMAGES');
		expect(remove).toBeDisabled();
		await fireEvent.click(remove);
		expect(fetcher).not.toHaveBeenCalled();
		await typeName('images');
		await fireEvent.click(remove);
		await waitFor(() => expect(nav.goto).toHaveBeenCalledWith('/storage'));
		expect(refresh).toHaveBeenCalledWith({ queryKey: keys.pools });
		expect(fetcher.mock.calls[0][1]?.method).toBe('DELETE');
		expect(body(fetcher)).toEqual({ confirm: 'images', delete_files: false });
	});

	it('deletes the files when the box is ticked', async () => {
		const { fetcher } = show(pool);
		await fireEvent.click(button('Remove images'));
		await fireEvent.click(screen.getByLabelText('Also delete every volume in the pool'));
		await fireEvent.input(screen.getByLabelText('Type images to remove it'), {
			target: { value: 'images' }
		});
		await fireEvent.click(button('Remove'));
		await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
		expect(body(fetcher)).toEqual({ confirm: 'images', delete_files: true });
	});

	it('shows a failure and stays on the page', async () => {
		show(
			pool,
			() => new Response(JSON.stringify({ error: 'the pool is not running' }), { status: 409 })
		);
		await fireEvent.click(button('Stop'));
		expect(await screen.findByRole('alert')).toHaveTextContent('The pool is not running.');
	});
});
