import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import Page from './+page.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { testClient } from '$lib/test/fixtures';
import { keys, type Network, type NetworkDetail } from '$lib/api';

const params = vi.hoisted(() => ({ name: 'lab' }));
vi.mock('$app/state', () => ({ page: { params } }));
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));

const listed: Network = {
	uuid: '00000000-0000-0000-0000-00000000000c',
	name: 'lab',
	active: true,
	persistent: true,
	autostart: true,
	bridge: 'virbr1'
};

const detail: NetworkDetail = {
	...listed,
	mode: 'nat',
	subnets: ['192.168.150.0/24'],
	used_by: []
};

function show(setup: (client: ReturnType<typeof testClient>) => void) {
	const client = testClient();
	setup(client);
	return render(QueryHarness, { props: { client, component: Page, props: {} } });
}

const value = (label: string) => screen.getByText(label).nextElementSibling;

beforeEach(() => {
	params.name = 'lab';
});

describe('a network page', () => {
	it.each([
		['nat', 'NAT'],
		[null, 'Isolated'],
		['bridge', 'Host bridge'],
		['route', 'route']
	])('shows the facts of a %s network', (mode, text) => {
		show((c) => {
			c.setQueryData(keys.networks, [listed]);
			c.setQueryData(keys.network(listed.uuid), { ...detail, mode });
		});
		expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent('lab');
		expect(value('Kind')).toHaveTextContent(text);
		expect(value('Subnets')).toHaveTextContent('192.168.150.0/24');
		expect(value('Bridge')).toHaveTextContent('virbr1');
		expect(value('Autostart')).toHaveTextContent('On');
	});

	it('shows dashes for a stopped bridge network without subnets', () => {
		show((c) => {
			c.setQueryData(keys.networks, [{ ...listed, active: false, bridge: null }]);
			c.setQueryData(keys.network(listed.uuid), {
				...detail,
				active: false,
				bridge: null,
				mode: 'bridge',
				subnets: []
			});
		});
		expect(value('State')).toHaveTextContent('inactive');
		expect(value('Subnets')).toHaveTextContent('–');
		expect(value('Bridge')).toHaveTextContent('–');
	});

	it('says so when libvirt has no network of that name', () => {
		params.name = 'gone';
		show((c) => c.setQueryData(keys.networks, [listed]));
		expect(screen.getByRole('alert')).toHaveTextContent(
			'libvirt has no virtual network called gone.'
		);
	});

	it('shows the loading and a failed detail', async () => {
		vi.stubGlobal(
			'fetch',
			vi.fn(async () => new Response(null, { status: 502, statusText: 'Bad Gateway' }))
		);
		show((c) => c.setQueryData(keys.networks, [listed]));
		expect(screen.getByText('Loading the network…')).toBeInTheDocument();
		expect(await screen.findByRole('alert')).toHaveTextContent('Could not load the network');
		vi.unstubAllGlobals();
	});

	it('shows a failed list', async () => {
		vi.stubGlobal(
			'fetch',
			vi.fn(async () => new Response(null, { status: 500, statusText: 'Internal Server Error' }))
		);
		show(() => {});
		expect(await screen.findByRole('alert')).toHaveTextContent('Could not load the networks');
		vi.unstubAllGlobals();
	});
});
