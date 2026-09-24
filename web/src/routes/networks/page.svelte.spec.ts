import { describe, expect, it, vi } from 'vitest';
import { render, screen, within } from '@testing-library/svelte';
import Page from './+page.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { testClient } from '$lib/test/fixtures';
import { keys, type Network } from '$lib/api';

vi.mock('$app/navigation', () => ({ goto: vi.fn() }));

const networks: Network[] = [
	{
		uuid: '00000000-0000-0000-0000-00000000000c',
		name: 'default',
		active: true,
		persistent: true,
		autostart: true,
		bridge: 'virbr0'
	},
	{
		uuid: '00000000-0000-0000-0000-00000000000d',
		name: 'lan',
		active: false,
		persistent: true,
		autostart: false,
		bridge: null
	}
];

function show(list: Network[]) {
	const client = testClient();
	client.setQueryData(keys.networks, list);
	client.setQueryData(keys.hostBridges, []);
	return render(QueryHarness, { props: { client, component: Page, props: {} } });
}

describe('the networks page', () => {
	it('lists each network with its state, bridge, and autostart, and links its page', () => {
		show(networks);
		const rows = screen.getAllByRole('row');
		expect(rows).toHaveLength(3);
		const first = within(rows[1]);
		expect(first.getByRole('link', { name: 'default' })).toHaveAttribute(
			'href',
			'/networks/default'
		);
		expect(first.getByText('running')).toBeInTheDocument();
		expect(first.getByText('virbr0')).toBeInTheDocument();
		expect(first.getByText('Yes')).toBeInTheDocument();
		const second = within(rows[2]);
		expect(second.getByText('inactive')).toBeInTheDocument();
		expect(second.getByText('–')).toBeInTheDocument();
	});

	it('says so when there is no network, and still offers the form', () => {
		show([]);
		expect(screen.getByText('libvirt has no virtual networks on this host.')).toBeInTheDocument();
		expect(screen.getByRole('button', { name: 'Create network' })).toBeInTheDocument();
	});

	it('shows the loading and the failed list', async () => {
		vi.stubGlobal(
			'fetch',
			vi.fn(async () => new Response(null, { status: 500, statusText: 'Internal Server Error' }))
		);
		const client = testClient();
		client.setQueryData(keys.hostBridges, []);
		render(QueryHarness, { props: { client, component: Page, props: {} } });
		expect(screen.getByText('Loading the networks…')).toBeInTheDocument();
		expect(await screen.findByRole('alert')).toHaveTextContent('Could not load the networks');
		vi.unstubAllGlobals();
	});
});
