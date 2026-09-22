// Types and fetchers for the Lodger REST API (crates/lodger/src/api.rs).

export type VmState =
	| 'no_state'
	| 'running'
	| 'blocked'
	| 'paused'
	| 'shutting_down'
	| 'shutoff'
	| 'crashed'
	| 'suspended'
	| 'unknown';

export interface Vm {
	uuid: string;
	name: string;
	state: VmState;
	vcpus: number;
	/** Current memory in KiB. */
	memory_kib: number;
	/** `false` for a transient domain, which disappears when it stops. */
	persistent: boolean;
	autostart: boolean;
}

export interface Connection {
	state: 'connecting' | 'connected' | 'disconnected';
	error?: string;
}

export interface Host {
	connection: Connection;
	info: {
		hostname: string;
		libvirt_version: string;
		cpus: number;
		memory_kib: number;
	} | null;
	info_error?: string;
	vms: { total: number; running: number };
	pools: number;
	networks: number;
}

/** The query keys, in one place so the events socket can invalidate them. */
export const keys = {
	host: ['host'] as const,
	vms: ['vms'] as const,
	vm: (id: string) => ['vms', id] as const
};

async function getJson<T>(path: string, fetcher: typeof fetch = fetch): Promise<T> {
	const res = await fetcher(path, { headers: { accept: 'application/json' } });
	if (!res.ok) {
		throw new Error(`${path} answered ${res.status} ${res.statusText}`);
	}
	return (await res.json()) as T;
}

export const fetchHost = (fetcher?: typeof fetch) => getJson<Host>('/api/host', fetcher);
export const fetchVms = (fetcher?: typeof fetch) => getJson<Vm[]>('/api/vms', fetcher);

/** Formats KiB with binary units, for example `4 GiB`. */
export function formatKib(kib: number): string {
	const units = ['KiB', 'MiB', 'GiB', 'TiB'];
	let value = kib;
	let unit = 0;
	while (value >= 1024 && unit < units.length - 1) {
		value /= 1024;
		unit += 1;
	}
	const rounded = Number.isInteger(value) ? value : Number(value.toFixed(1));
	return `${rounded} ${units[unit]}`;
}
