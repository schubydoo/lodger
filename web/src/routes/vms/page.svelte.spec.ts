import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/svelte';
import Page from './+page.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { testClient, vms } from '$lib/test/fixtures';
import { keys } from '$lib/api';
import { lastDeletion } from '$lib/deletion.svelte';

function show(setup: (client: ReturnType<typeof testClient>) => void = () => {}) {
	const client = testClient();
	setup(client);
	return render(QueryHarness, { props: { client, component: Page, props: {} } });
}

afterEach(() => {
	vi.unstubAllGlobals();
	lastDeletion.current = null;
});

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

	it('links each name to its VM page', () => {
		show((c) => c.setQueryData(keys.vms, vms));
		expect(screen.getByRole('link', { name: 'beta' })).toHaveAttribute('href', '/vms/beta');
	});

	it('links the console of each running VM, and only of running ones', () => {
		show((c) => c.setQueryData(keys.vms, vms));
		expect(screen.getByRole('link', { name: 'Console of alpha' })).toHaveAttribute(
			'href',
			'/vms/alpha/console'
		);
		expect(screen.queryByRole('link', { name: 'Console of beta' })).toBeNull();
	});

	it('offers the power actions that fit each VM state', () => {
		show((c) => c.setQueryData(keys.vms, vms));
		const [, alpha, beta] = screen.getAllByRole('row');
		expect(within(alpha).getByRole('button', { name: 'Shut down alpha' })).toBeInTheDocument();
		expect(within(alpha).getByRole('button', { name: 'Force off alpha' })).toBeInTheDocument();
		expect(within(beta).getByRole('button', { name: 'Start beta' })).toBeInTheDocument();
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

describe('the result of a delete', () => {
	it('lists the deleted and the kept volumes with each reason, until dismissed', async () => {
		lastDeletion.current = {
			name: 'gone',
			removal: {
				removed: ['/p/gone.qcow2'],
				skipped: [
					{ path: '/p/base.qcow2', reason: 'used_by', vm: 'alpha' },
					{ path: '/iso/seed.img', reason: 'shared' },
					{ path: '/srv/x.img', reason: 'not_in_pool' },
					{ path: '/p/busy.img', reason: 'failed', message: 'volume is busy' }
				]
			}
		};
		show((c) => c.setQueryData(keys.vms, vms));
		const note = screen.getByRole('status');
		expect(note).toHaveTextContent('Deleted gone. Deleted 1 volume.');
		expect(within(note).getByText('/p/gone.qcow2')).toBeInTheDocument();
		expect(within(note).getByText('/p/base.qcow2: alpha uses it')).toBeInTheDocument();
		expect(
			within(note).getByText('/iso/seed.img: it is read-only or shareable')
		).toBeInTheDocument();
		expect(within(note).getByText('/srv/x.img: no storage pool holds it')).toBeInTheDocument();
		expect(
			within(note).getByText('/p/busy.img: libvirt refused: volume is busy')
		).toBeInTheDocument();
		await fireEvent.click(within(note).getByRole('button', { name: 'Dismiss' }));
		expect(screen.queryByRole('status')).toBeNull();
	});

	it('says so when no volume was deleted', () => {
		lastDeletion.current = { name: 'gone', removal: { removed: [], skipped: [] } };
		show((c) => c.setQueryData(keys.vms, vms));
		expect(screen.getByRole('status')).toHaveTextContent('Deleted gone. No volume was deleted.');
	});
});
