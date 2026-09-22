// Test data shaped like the API answers (crates/lodger/src/api.rs).
import { QueryClient } from '@tanstack/svelte-query';
import type { Host, Vm } from '$lib/api';

/** A client that never refetches on its own, so a test sees only what it set. */
export function testClient(): QueryClient {
	return new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
}

export const host: Host = {
	connection: { state: 'connected' },
	info: { hostname: 'kvm01', libvirt_version: '11.3.0', cpus: 20, memory_kib: 65610412 },
	vms: { total: 3, running: 2 },
	pools: 1,
	networks: 2
};

export const vms: Vm[] = [
	{
		uuid: '00000000-0000-0000-0000-000000000001',
		name: 'alpha',
		state: 'running',
		vcpus: 2,
		memory_kib: 4194304,
		persistent: true,
		autostart: true
	},
	{
		uuid: '00000000-0000-0000-0000-000000000002',
		name: 'beta',
		state: 'shutoff',
		vcpus: 1,
		memory_kib: 1048576,
		persistent: false,
		autostart: false
	}
];
