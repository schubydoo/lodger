import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import Page from './+page.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { host, testClient } from '$lib/test/fixtures';
import { keys, type Host } from '$lib/api';

function show(data?: Host) {
	const client = testClient();
	if (data) client.setQueryData(keys.host, data);
	return render(QueryHarness, { props: { client, component: Page, props: {} } });
}

afterEach(() => vi.unstubAllGlobals());

describe('the host overview', () => {
	it('shows the counts and the host facts', () => {
		show(host);
		expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent('kvm01');
		expect(screen.getByText('2 running')).toHaveAttribute('href', '/vms');
		expect(screen.getByText('20 CPUs · 62.6 GiB')).toBeInTheDocument();
		expect(screen.getByText('libvirt 11.3.0')).toBeInTheDocument();
	});

	it('explains missing host facts', () => {
		show({ ...host, info: null, info_error: 'libvirt: timed out' });
		expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent('Host overview');
		expect(screen.getByText('Unknown')).toBeInTheDocument();
		expect(screen.getByText('libvirt: timed out')).toBeInTheDocument();
	});

	it('fetches /api/host and shows an error answer', async () => {
		vi.stubGlobal(
			'fetch',
			vi.fn(async () => new Response('', { status: 500 }))
		);
		show();
		expect(screen.getByText(/Loading/)).toBeInTheDocument();
		expect(await screen.findByRole('alert')).toHaveTextContent('/api/host answered 500');
	});
});
