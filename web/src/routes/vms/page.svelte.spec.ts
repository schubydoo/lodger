import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, within } from '@testing-library/svelte';
import Page from './+page.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { testClient, vms } from '$lib/test/fixtures';
import { keys } from '$lib/api';

function show(setup: (client: ReturnType<typeof testClient>) => void = () => {}) {
	const client = testClient();
	setup(client);
	return render(QueryHarness, { props: { client, component: Page, props: {} } });
}

afterEach(() => vi.unstubAllGlobals());

describe('the VM list', () => {
	it('lists each VM with its state, size, and autostart', () => {
		show((c) => c.setQueryData(keys.vms, vms));
		const rows = screen.getAllByRole('row');
		expect(rows).toHaveLength(3);
		const alpha = within(rows[1]);
		expect(alpha.getByText('alpha')).toBeInTheDocument();
		expect(alpha.getByText('Running')).toBeInTheDocument();
		expect(alpha.getByText('4 GiB')).toBeInTheDocument();
		expect(alpha.getByText('Yes')).toBeInTheDocument();
		const beta = within(rows[2]);
		expect(beta.getByText('Shut off')).toBeInTheDocument();
		expect(beta.getByText('(transient)')).toBeInTheDocument();
	});

	it('links the console of each running VM, and only of running ones', () => {
		show((c) => c.setQueryData(keys.vms, vms));
		expect(screen.getByRole('link', { name: 'Console of alpha' })).toHaveAttribute(
			'href',
			'/vms/alpha/console'
		);
		expect(screen.queryByRole('link', { name: 'Console of beta' })).toBeNull();
	});

	it('puts the table in a named region that a keyboard can focus', () => {
		show((c) => c.setQueryData(keys.vms, vms));
		const region = screen.getByRole('region', { name: 'Virtual machines' });
		expect(region).toHaveAttribute('tabindex', '0');
	});

	it('says so when libvirt has no VMs', () => {
		show((c) => c.setQueryData(keys.vms, []));
		expect(screen.getByText(/has no virtual machines/)).toBeInTheDocument();
	});

	it('fetches /api/vms and shows an error answer', async () => {
		vi.stubGlobal(
			'fetch',
			vi.fn(async () => new Response('', { status: 503, statusText: 'Service Unavailable' }))
		);
		show();
		expect(screen.getByText(/Loading/)).toBeInTheDocument();
		expect(await screen.findByRole('alert')).toHaveTextContent('/api/vms answered 503');
	});
});
