import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import Page from './+page.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { testClient } from '$lib/test/fixtures';
import { keys, type Pool, type PoolDetail } from '$lib/api';

const params = vi.hoisted(() => ({ name: 'nas' }));
vi.mock('$app/state', () => ({ page: { params } }));
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));

const listed: Pool = {
	uuid: '00000000-0000-0000-0000-00000000000b',
	name: 'nas',
	state: 'running',
	capacity_bytes: 4 * 1024 ** 4,
	allocation_bytes: 1024 ** 4,
	available_bytes: 3 * 1024 ** 4,
	persistent: true,
	autostart: true
};

const detail: PoolDetail = {
	...listed,
	kind: 'netfs',
	path: '/var/lib/libvirt/pools/nas',
	nfs: { host: 'nas.lan', export: '/volume1/vm' },
	used_by: []
};

function show(setup: (client: ReturnType<typeof testClient>) => void) {
	const client = testClient();
	setup(client);
	return render(QueryHarness, { props: { client, component: Page, props: {} } });
}

beforeEach(() => {
	params.name = 'nas';
});

describe('a pool page', () => {
	it('shows the facts of an NFS pool', () => {
		show((c) => {
			c.setQueryData(keys.pools, [listed]);
			c.setQueryData(keys.pool(listed.uuid), detail);
			c.setQueryData(keys.volumes(listed.uuid), []);
		});
		expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent('nas');
		expect(screen.getByText('Kind').nextElementSibling).toHaveTextContent('NFS share');
		expect(screen.getByText('nas.lan:/volume1/vm')).toBeInTheDocument();
		expect(screen.getByText('/var/lib/libvirt/pools/nas')).toBeInTheDocument();
		expect(screen.getByText('1 TiB used of 4 TiB')).toBeInTheDocument();
		expect(screen.getByText('On')).toBeInTheDocument();
		expect(screen.getByRole('button', { name: 'Stop' })).toBeInTheDocument();
		// A running pool shows its volumes.
		expect(screen.getByRole('heading', { name: 'Volumes' })).toBeInTheDocument();
		expect(screen.getByText('nas has no volumes.')).toBeInTheDocument();
	});

	it('shows a folder pool without sizes while it is stopped', () => {
		show((c) => {
			c.setQueryData(keys.pools, [{ ...listed, state: 'inactive', autostart: false }]);
			c.setQueryData(keys.pool(listed.uuid), {
				...detail,
				state: 'inactive',
				autostart: false,
				kind: 'dir',
				path: '/srv/vm',
				nfs: undefined
			});
		});
		expect(screen.getByText('Kind').nextElementSibling).toHaveTextContent('Folder');
		expect(screen.queryByText(/used of/)).toBeNull();
		expect(screen.getByText('Off')).toBeInTheDocument();
		expect(screen.getByRole('button', { name: 'Start' })).toBeInTheDocument();
		// libvirt lists no volumes of a stopped pool.
		expect(screen.queryByRole('heading', { name: 'Volumes' })).toBeNull();
		expect(screen.getByText('Start the pool to see and change its volumes.')).toBeInTheDocument();
	});

	it('says so when libvirt has no pool of that name', () => {
		params.name = 'gone';
		show((c) => c.setQueryData(keys.pools, [listed]));
		expect(screen.getByRole('alert')).toHaveTextContent('libvirt has no storage pool called gone.');
	});
});
