import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import Page from './+page.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { testClient, vms } from '$lib/test/fixtures';
import { keys, type VmStats } from '$lib/api';

const params = vi.hoisted(() => ({ name: 'alpha' }));
vi.mock('$app/state', () => ({ page: { params } }));

// The page asks for live stats while it is open. The spy records the
// requests and their releases.
const interest = vi.hoisted(() => ({ wanted: 0, released: 0 }));
vi.mock('$lib/events', () => ({
	wantStats: () => {
		interest.wanted += 1;
		return () => (interest.released += 1);
	}
}));

const [alpha] = vms;

const stats: VmStats = {
	uuid: alpha.uuid,
	cpu_percent: 42.25,
	memory_kib: 4194304,
	memory_used_kib: 1048576,
	disk_read_bps: 1500,
	disk_write_bps: null,
	net_rx_bps: 2_000_000,
	net_tx_bps: 12
};

function show(name: string, live?: VmStats[]) {
	params.name = name;
	const client = testClient();
	client.setQueryData(keys.vms, vms);
	if (live) client.setQueryData(keys.stats, live);
	return render(QueryHarness, { props: { client, component: Page, props: {} } });
}

// Testing Library unmounts the page of the previous test after the test ends,
// which counts as a release, so the counters start fresh before each test.
beforeEach(() => {
	interest.wanted = 0;
	interest.released = 0;
});

describe('the VM page', () => {
	it('shows the live values of a running VM', () => {
		show('alpha', [stats]);
		expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent('alpha');
		expect(screen.getByText('42.3 %')).toBeInTheDocument();
		expect(screen.getByText('of 2 vCPUs')).toBeInTheDocument();
		expect(screen.getByText('1 GiB of 4 GiB')).toBeInTheDocument();
		expect(screen.getByText('Read 1.5 kB/s')).toBeInTheDocument();
		expect(screen.getByText('Write –')).toBeInTheDocument();
		expect(screen.getByText('Received 2 MB/s')).toBeInTheDocument();
		expect(screen.getByText('Sent 12 B/s')).toBeInTheDocument();
		expect(screen.getByRole('link', { name: 'Console' })).toHaveAttribute(
			'href',
			'/vms/alpha/console'
		);
	});

	it('shows the memory size when the guest reports no use', () => {
		show('alpha', [{ ...stats, memory_used_kib: null }]);
		expect(screen.getByText('4 GiB (the guest reports no use)')).toBeInTheDocument();
	});

	it('waits for the first values, and ignores other VMs', () => {
		show('alpha', [{ ...stats, uuid: vms[1].uuid }]);
		expect(screen.getByRole('status')).toHaveTextContent('Waiting for the first values');
	});

	it('explains that a stopped VM has no live values', () => {
		show('beta', [stats]);
		expect(screen.getByText('Live values show while the VM runs.')).toBeInTheDocument();
		expect(screen.queryByRole('link', { name: 'Console' })).toBeNull();
	});

	it('says so for an unknown name', () => {
		show('gamma');
		expect(screen.getByRole('alert')).toHaveTextContent('no virtual machine called gamma');
	});

	it('asks for live stats while it is open, and stops when it closes', () => {
		const view = show('alpha');
		expect(interest).toEqual({ wanted: 1, released: 0 });
		view.unmount();
		expect(interest).toEqual({ wanted: 1, released: 1 });
	});
});
