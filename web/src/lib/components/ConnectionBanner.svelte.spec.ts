import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import ConnectionBanner from './ConnectionBanner.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { host, testClient } from '$lib/test/fixtures';
import { keys, type Host } from '$lib/api';

function show(data: Host, socketOpen = true) {
	const client = testClient();
	client.setQueryData(keys.host, data);
	return render(QueryHarness, {
		props: { client, component: ConnectionBanner, props: { socketOpen } }
	});
}

describe('ConnectionBanner', () => {
	it('shows nothing while everything is connected', () => {
		show(host);
		expect(screen.getByRole('alert')).toBeEmptyDOMElement();
	});

	it('names the libvirt error while libvirt is disconnected', () => {
		show({ ...host, connection: { state: 'disconnected', error: 'libvirtd is gone' } });
		expect(screen.getByText('libvirt is not connected')).toBeInTheDocument();
		expect(screen.getByText(/libvirtd is gone\. Lodger tries again/)).toBeInTheDocument();
	});

	it('says so while Lodger connects to libvirt', () => {
		show({ ...host, connection: { state: 'connecting' } });
		expect(screen.getByText('Connecting to libvirt')).toBeInTheDocument();
	});

	it('says so when the page cannot reach the Lodger server', () => {
		show(host, false);
		expect(screen.getByText('Lodger is not reachable')).toBeInTheDocument();
	});

	it('has one alert region, not one inside another', () => {
		show(host, false);
		expect(screen.getAllByRole('alert')).toHaveLength(1);
	});
});
