import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import NetworkManage from './NetworkManage.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { testClient } from '$lib/test/fixtures';
import { keys, type NetworkDetail } from '$lib/api';

const nav = vi.hoisted(() => ({ goto: vi.fn(async () => {}) }));
vi.mock('$app/navigation', () => ({ goto: nav.goto }));

const network: NetworkDetail = {
	uuid: '00000000-0000-0000-0000-00000000000c',
	name: 'lab',
	active: true,
	persistent: true,
	autostart: true,
	bridge: 'virbr1',
	mode: 'nat',
	subnets: ['192.168.150.0/24'],
	used_by: []
};

function show(
	n: NetworkDetail,
	respond: () => Response = () => new Response(null, { status: 204 })
) {
	const fetcher = vi.fn<typeof fetch>(async () => respond());
	vi.stubGlobal('fetch', fetcher);
	const client = testClient();
	client.setQueryData(keys.session, { username: 'admin', csrf_token: 'csrf1' });
	render(QueryHarness, { props: { client, component: NetworkManage, props: { network: n } } });
	return { fetcher };
}

const button = (name: string) => screen.getByRole('button', { name });
const body = (fetcher: ReturnType<typeof show>['fetcher']) =>
	JSON.parse(fetcher.mock.calls[0][1]?.body as string);

beforeEach(() => nav.goto.mockClear());
afterEach(() => vi.unstubAllGlobals());

describe('a network page', () => {
	it('stops a running network and starts a stopped one', async () => {
		const { fetcher } = show(network);
		await fireEvent.click(button('Stop'));
		await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
		expect(fetcher.mock.calls[0][0]).toBe(`/api/networks/${network.uuid}`);
		expect(fetcher.mock.calls[0][1]?.method).toBe('PATCH');
		expect(body(fetcher)).toEqual({ active: false });
		const second = show({ ...network, active: false });
		await fireEvent.click(button('Start'));
		await waitFor(() => expect(second.fetcher).toHaveBeenCalledOnce());
		expect(body(second.fetcher)).toEqual({ active: true });
	});

	it('switches autostart', async () => {
		const { fetcher } = show(network);
		await fireEvent.click(button('Turn autostart off'));
		await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
		expect(body(fetcher)).toEqual({ autostart: false });
	});

	it('lists the VMs on the network before a delete', () => {
		show({ ...network, used_by: ['db'] });
		expect(screen.getByRole('note')).toHaveTextContent('These VMs have a NIC on lab.');
		expect(screen.getByRole('note')).toHaveTextContent('db');
	});

	it('deletes only after the exact name is typed', async () => {
		const { fetcher } = show(network);
		await fireEvent.click(button('Delete lab'));
		const remove = button('Delete');
		const typeName = (value: string) =>
			fireEvent.input(screen.getByLabelText('Type lab to delete it'), { target: { value } });
		await typeName('LAB');
		await fireEvent.click(remove);
		expect(fetcher).not.toHaveBeenCalled();
		await typeName('lab');
		await fireEvent.click(remove);
		await waitFor(() => expect(nav.goto).toHaveBeenCalledWith('/networks'));
		expect(fetcher.mock.calls[0][1]?.method).toBe('DELETE');
		expect(body(fetcher)).toEqual({ confirm: 'lab' });
	});

	it('shows a failure and stays on the page', async () => {
		show(
			network,
			() => new Response(JSON.stringify({ error: 'the network is not running' }), { status: 409 })
		);
		await fireEvent.click(button('Stop'));
		expect(await screen.findByRole('alert')).toHaveTextContent('The network is not running.');
	});
});
