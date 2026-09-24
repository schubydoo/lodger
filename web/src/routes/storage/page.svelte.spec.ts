import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, within } from '@testing-library/svelte';
import Page from './+page.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { testClient } from '$lib/test/fixtures';
import { keys, type Pool } from '$lib/api';

vi.mock('$app/navigation', () => ({ goto: vi.fn() }));

const pools: Pool[] = [
	{
		uuid: '00000000-0000-0000-0000-00000000000a',
		name: 'images',
		state: 'running',
		capacity_bytes: 2 * 1024 ** 4,
		allocation_bytes: 1024 ** 4,
		available_bytes: 1024 ** 4,
		persistent: true,
		autostart: true
	},
	{
		uuid: '00000000-0000-0000-0000-00000000000b',
		name: 'nas',
		state: 'inactive',
		capacity_bytes: 0,
		allocation_bytes: 0,
		available_bytes: 0,
		persistent: true,
		autostart: false
	}
];

function show(list: Pool[]) {
	const client = testClient();
	client.setQueryData(keys.pools, list);
	return render(QueryHarness, { props: { client, component: Page, props: {} } });
}

afterEach(() => vi.unstubAllGlobals());

describe('the storage page', () => {
	it('lists each pool with its state, size, and autostart, and links its page', () => {
		show(pools);
		const rows = screen.getAllByRole('row');
		expect(rows).toHaveLength(3);
		const images = within(rows[1]);
		expect(images.getByRole('link', { name: 'images' })).toHaveAttribute('href', '/storage/images');
		expect(images.getByText('running')).toBeInTheDocument();
		expect(images.getByText('2 TiB')).toBeInTheDocument();
		expect(images.getByText('Yes')).toBeInTheDocument();
		// An inactive pool reports no sizes.
		expect(within(rows[2]).getAllByText('–')).toHaveLength(2);
	});

	it('says so when there is no pool, and still offers the form', () => {
		show([]);
		expect(screen.getByText('libvirt has no storage pools on this host.')).toBeInTheDocument();
		expect(screen.getByRole('button', { name: 'Create pool' })).toBeInTheDocument();
	});
});
