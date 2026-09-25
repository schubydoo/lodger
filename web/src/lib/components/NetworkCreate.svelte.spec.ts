import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import NetworkCreate from './NetworkCreate.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { testClient } from '$lib/test/fixtures';
import { keys, type HostBridge } from '$lib/api';

const nav = vi.hoisted(() => ({ goto: vi.fn(async () => {}) }));
vi.mock('$app/navigation', () => ({ goto: nav.goto }));

function show(
	bridges: HostBridge[] | null = [
		{ name: 'br0', owner: null },
		{ name: 'virbr0', owner: 'libvirt network default' }
	],
	respond: () => Response = () => new Response(JSON.stringify({ uuid: 'u1' }), { status: 201 })
) {
	const fetcher = vi.fn<typeof fetch>(async () => respond());
	vi.stubGlobal('fetch', fetcher);
	const client = testClient();
	client.setQueryData(keys.session, { username: 'admin', csrf_token: 'csrf1' });
	if (bridges !== null) client.setQueryData(keys.hostBridges, bridges);
	render(QueryHarness, { props: { client, component: NetworkCreate, props: {} } });
	return { fetcher, client };
}

const type = (label: string, value: string) =>
	fireEvent.input(screen.getByLabelText(label), { target: { value } });
const sent = (fetcher: ReturnType<typeof show>['fetcher']) =>
	JSON.parse(fetcher.mock.calls[0][1]?.body as string);

beforeEach(() => nav.goto.mockClear());
afterEach(() => vi.unstubAllGlobals());

describe('the New network form', () => {
	it('creates a NAT network with autostart and opens its page', async () => {
		const { fetcher } = show();
		const create = screen.getByRole('button', { name: 'Create network' });
		expect(create).toBeDisabled();
		await type('Name', 'lab');
		expect(create).toBeDisabled();
		await type('Subnet', '192.168.150.0/24');
		await fireEvent.click(create);
		await waitFor(() => expect(nav.goto).toHaveBeenCalledWith('/networks/lab'));
		expect(fetcher.mock.calls[0][0]).toBe('/api/networks');
		expect((fetcher.mock.calls[0][1]?.headers as Record<string, string>)['x-csrf-token']).toBe(
			'csrf1'
		);
		expect(sent(fetcher)).toEqual({
			mode: 'nat',
			name: 'lab',
			subnet: '192.168.150.0/24',
			autostart: true
		});
	});

	it('creates an isolated network with autostart off', async () => {
		const { fetcher } = show();
		await fireEvent.click(screen.getByLabelText('Isolated'));
		await type('Name', 'iso');
		await type('Subnet', '10.77.0.0/24');
		await fireEvent.click(screen.getByLabelText('Start the network when the host boots'));
		await fireEvent.click(screen.getByRole('button', { name: 'Create network' }));
		await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
		expect(sent(fetcher)).toEqual({
			mode: 'isolated',
			name: 'iso',
			subnet: '10.77.0.0/24',
			autostart: false
		});
	});

	it('creates a bridge network on a host bridge from the list', async () => {
		const { fetcher } = show();
		await fireEvent.click(screen.getByLabelText('Host bridge'));
		await type('Name', 'lan');
		const create = screen.getByRole('button', { name: 'Create network' });
		expect(create).toBeDisabled();
		await fireEvent.change(screen.getByLabelText('Host bridge', { selector: 'select' }), {
			target: { value: 'br0' }
		});
		await fireEvent.click(create);
		await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
		expect(sent(fetcher)).toEqual({ mode: 'bridge', name: 'lan', bridge: 'br0', autostart: true });
	});

	it('offers only the bridges that nothing owns, and the others on request', async () => {
		show();
		await fireEvent.click(screen.getByLabelText('Host bridge'));
		const options = () =>
			[...screen.getByLabelText('Host bridge', { selector: 'select' }).querySelectorAll('option')]
				.map((o) => o.textContent)
				.slice(1);
		// libvirt's own bridge is hidden at first (D5).
		expect(options()).toEqual(['br0']);
		await fireEvent.click(screen.getByLabelText(/Show every bridge/));
		expect(options()).toEqual(['br0', 'virbr0 (libvirt network default)']);
	});

	it('explains when every bridge belongs to libvirt or Docker', async () => {
		show([
			{ name: 'docker0', owner: 'Docker' },
			{ name: 'br-1a2b3c4d5e6f', owner: 'Docker' }
		]);
		await fireEvent.click(screen.getByLabelText('Host bridge'));
		expect(screen.getByRole('note')).toHaveTextContent('belongs to a libvirt network or to Docker');
		await fireEvent.click(screen.getByLabelText(/Show every bridge/));
		expect(screen.getByLabelText('Host bridge', { selector: 'select' })).toHaveTextContent(
			'docker0 (Docker)'
		);
	});

	it('explains that a host bridge must exist first when the host has none', async () => {
		show([]);
		await fireEvent.click(screen.getByLabelText('Host bridge'));
		expect(screen.getByRole('note')).toHaveTextContent('A host bridge must exist first');
		expect(screen.getByRole('note')).toHaveTextContent('Lodger never changes the host');
		expect(screen.getByRole('button', { name: 'Create network' })).toBeDisabled();
	});

	it('shows an overlap and stays on the form', async () => {
		show(
			undefined,
			() =>
				new Response(
					JSON.stringify({
						error: 'Subnet overlaps 192.168.122.0/24, which network "default" uses'
					}),
					{ status: 422 }
				)
		);
		await type('Name', 'lab');
		await type('Subnet', '192.168.122.0/24');
		await fireEvent.click(screen.getByRole('button', { name: 'Create network' }));
		expect(await screen.findByRole('alert')).toHaveTextContent('which network "default" uses');
		expect(nav.goto).not.toHaveBeenCalled();
	});

	it('refreshes the network list before it opens the new page', async () => {
		const { client } = show();
		const refresh = vi.spyOn(client, 'invalidateQueries');
		await type('Name', 'lab');
		await type('Subnet', '192.168.150.0/24');
		await fireEvent.click(screen.getByRole('button', { name: 'Create network' }));
		await waitFor(() => expect(nav.goto).toHaveBeenCalled());
		expect(refresh).toHaveBeenCalledWith({ queryKey: keys.networks });
		expect(refresh.mock.invocationCallOrder[0]).toBeLessThan(nav.goto.mock.invocationCallOrder[0]);
	});

	it('shows a failed list of host bridges', async () => {
		show(null, () => new Response(null, { status: 500, statusText: 'Internal Server Error' }));
		await fireEvent.click(screen.getByLabelText('Host bridge'));
		expect(await screen.findByRole('alert')).toHaveTextContent('Could not load the host bridges');
	});
});
