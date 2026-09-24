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

/**
 * The live stats of one running VM (crates/lodger-core/src/model/stats.rs).
 * A value is `null` when libvirt did not report it. The rates are `null`
 * until a second sample exists.
 */
export interface VmStats {
	uuid: string;
	/** CPU use as a share of the VM's vCPUs: 100 means every vCPU is busy. */
	cpu_percent: number | null;
	/** Memory that the guest has now, in KiB. */
	memory_kib: number | null;
	/** Memory that the guest uses, in KiB, from the guest's own view. */
	memory_used_kib: number | null;
	/** Bytes per second. */
	disk_read_bps: number | null;
	disk_write_bps: number | null;
	net_rx_bps: number | null;
	net_tx_bps: number | null;
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

/** The logged-in user (crates/lodger/src/auth.rs). */
export interface Session {
	username: string;
	/** Goes in the `X-CSRF-Token` header of every state-changing request. */
	csrf_token: string;
}

/** An account as the account page shows it (crates/lodger/src/accounts.rs). */
export interface Account {
	id: number;
	username: string;
	created_at: string;
	password_changed_at: string;
	/** The caller's own account. */
	you: boolean;
}

/** The query keys, in one place so the events socket can invalidate them. */
export const keys = {
	host: ['host'] as const,
	vms: ['vms'] as const,
	vm: (id: string) => ['vms', id] as const,
	networks: ['networks'] as const,
	network: (id: string) => ['networks', id] as const,
	hostBridges: ['host-bridges'] as const,
	pools: ['pools'] as const,
	pool: (id: string) => ['pools', id] as const,
	/** Below the pool's key, so a pool event refreshes its volumes too. */
	volumes: (poolId: string) => ['pools', poolId, 'volumes'] as const,
	session: ['session'] as const,
	setup: ['setup'] as const,
	accounts: ['accounts'] as const,
	/** Live stats. The events socket fills it; nothing fetches it. */
	stats: ['stats'] as const
};

/**
 * The cause and the fix of a known libvirt error
 * (`crates/lodger-virt/src/errors.rs`). `commands` run on the host.
 */
export interface Explanation {
	cause: string;
	fix: string;
	commands: string[];
}

/** An error answer, with its status so the app can react to a 401. */
export class ApiError extends Error {
	constructor(
		readonly status: number,
		message: string,
		readonly explanation?: Explanation
	) {
		super(message);
	}
}

/** The explanation that an error answer carried, if any. */
export function explanationOf(error: unknown): Explanation | undefined {
	return error instanceof ApiError ? error.explanation : undefined;
}

/** Reads `cause`, `fix`, and `commands` from an error body, if all are there. */
function readExplanation(data: unknown): Explanation | undefined {
	const d = data as Partial<Record<keyof Explanation, unknown>> | null;
	if (
		typeof d?.cause === 'string' &&
		typeof d.fix === 'string' &&
		Array.isArray(d.commands) &&
		d.commands.every((c) => typeof c === 'string')
	) {
		return { cause: d.cause, fix: d.fix, commands: d.commands };
	}
	return undefined;
}

async function getJson<T>(path: string, fetcher: typeof fetch = fetch): Promise<T> {
	const res = await fetcher(path, { headers: { accept: 'application/json' } });
	if (!res.ok) {
		throw new ApiError(res.status, `${path} answered ${res.status} ${res.statusText}`);
	}
	return (await res.json()) as T;
}

/**
 * Sends a state-changing request. The error answers carry a sentence in
 * `error`, which the pages show as it is.
 */
export async function send<T>(
	method: 'POST' | 'PATCH' | 'DELETE',
	path: string,
	options: { body?: unknown; csrf?: string; fetcher?: typeof fetch } = {}
): Promise<T> {
	const headers: Record<string, string> = { accept: 'application/json' };
	if (options.body !== undefined) headers['content-type'] = 'application/json';
	if (options.csrf) headers['x-csrf-token'] = options.csrf;
	const res = await (options.fetcher ?? fetch)(path, {
		method,
		headers,
		body: options.body === undefined ? undefined : JSON.stringify(options.body)
	});
	const text = await res.text();
	let data: unknown = null;
	try {
		data = text ? JSON.parse(text) : null;
	} catch {
		// Not JSON, for example a proxy's error page: keep `null`.
	}
	if (!res.ok) {
		const message = (data as { error?: unknown } | null)?.error;
		throw new ApiError(
			res.status,
			typeof message === 'string' ? message : `${path} answered ${res.status} ${res.statusText}`,
			readExplanation(data)
		);
	}
	return data as T;
}

/**
 * The text to show for a failure. The server writes its messages as lower-case
 * clauses, so this makes each one a sentence.
 */
export function problemText(error: unknown): string {
	const text = (error instanceof Error ? error.message : String(error)).trim();
	if (!text) return 'Something went wrong.';
	const sentence = text[0].toUpperCase() + text.slice(1);
	return /[.!?]$/.test(sentence) ? sentence : `${sentence}.`;
}

/** A power action on a VM (`crates/lodger-virt/src/power.rs`). */
export type VmAction = 'start' | 'shutdown' | 'force-off' | 'reboot' | 'pause' | 'resume';

/**
 * Asks for a power action. A 204 means that libvirt took the call: the new
 * state arrives through the events socket, which refreshes the VM list.
 * Force off needs `confirm`, the VM's name as the user typed it.
 */
export function vmAction(
	id: string,
	action: VmAction,
	options: { confirm?: string; csrf?: string; fetcher?: typeof fetch } = {}
): Promise<null> {
	return send<null>('POST', `/api/vms/${id}/actions/${action}`, {
		body: options.confirm === undefined ? undefined : { confirm: options.confirm },
		csrf: options.csrf,
		fetcher: options.fetcher
	});
}

/** Switches autostart. The inventory refreshes through the events socket. */
export function setAutostart(
	id: string,
	autostart: boolean,
	options: { csrf?: string; fetcher?: typeof fetch } = {}
): Promise<null> {
	return send<null>('PATCH', `/api/vms/${id}`, { body: { autostart }, ...options });
}

/** A disk that a delete kept (`crates/lodger/src/actions.rs`). */
export interface SkippedDisk {
	path: string;
	reason: 'used_by' | 'shared' | 'not_in_pool' | 'failed';
	/** For `used_by`: the VM that uses the disk. */
	vm?: string;
	/** For `failed`: libvirt's message. */
	message?: string;
}

/** What a delete did with the VM's disks. */
export interface Removal {
	removed: string[];
	skipped: SkippedDisk[];
}

/**
 * Deletes a shut-off VM. `confirm` is the VM's name as the user typed it.
 * With `removeVolumes`, libvirt also deletes the volumes that no other VM
 * uses.
 */
export function deleteVm(
	id: string,
	options: { confirm: string; removeVolumes: boolean; csrf?: string; fetcher?: typeof fetch }
): Promise<Removal> {
	return send<Removal>('DELETE', `/api/vms/${id}`, {
		body: { confirm: options.confirm, remove_volumes: options.removeVolumes },
		csrf: options.csrf,
		fetcher: options.fetcher
	});
}

/** Why a delete kept a disk, as a clause for a sentence. */
export function skipText(disk: SkippedDisk): string {
	switch (disk.reason) {
		case 'used_by':
			return `${disk.vm} uses it`;
		case 'shared':
			return 'it is read-only or shareable';
		case 'not_in_pool':
			return 'no storage pool holds it';
		case 'failed':
			return `libvirt refused: ${disk.message}`;
	}
}

/** The session, or `null` when nobody is logged in. */
export async function fetchSession(fetcher: typeof fetch = fetch): Promise<Session | null> {
	try {
		return await getJson<Session>('/api/session', fetcher);
	} catch (e) {
		if (e instanceof ApiError && e.status === 401) return null;
		throw e;
	}
}

/** Whether setup is open, that is, no account exists yet. */
export async function fetchSetupOpen(fetcher: typeof fetch = fetch): Promise<boolean> {
	try {
		await getJson<unknown>('/api/setup', fetcher);
		return true;
	} catch (e) {
		if (e instanceof ApiError && e.status === 404) return false;
		throw e;
	}
}

export const fetchAccounts = (fetcher?: typeof fetch) =>
	getJson<Account[]>('/api/accounts', fetcher);

export const fetchHost = (fetcher?: typeof fetch) => getJson<Host>('/api/host', fetcher);
export const fetchVms = (fetcher?: typeof fetch) => getJson<Vm[]>('/api/vms', fetcher);

/** A virtual network, as `GET /api/networks` lists it. */
export interface Network {
	uuid: string;
	name: string;
	active: boolean;
	persistent: boolean;
	autostart: boolean;
	/** The bridge device on the host, such as `virbr0`. */
	bridge: string | null;
}

/** One network with its mode, subnets, and users (`crates/lodger/src/networks.rs`). */
export interface NetworkDetail extends Network {
	/** `nat`, `bridge`, or another forward mode. `null` means isolated. */
	mode: string | null;
	/** The IPv4 subnets, such as `192.168.122.0/24`. */
	subnets: string[];
	/** The VMs with a NIC on the network. */
	used_by: string[];
}

export const fetchNetworks = (fetcher?: typeof fetch) =>
	getJson<Network[]>('/api/networks', fetcher);
export const fetchNetwork = (id: string, fetcher?: typeof fetch) =>
	getJson<NetworkDetail>(`/api/networks/${id}`, fetcher);
/** The bridges on the host. Lodger only reads them. */
export const fetchHostBridges = (fetcher?: typeof fetch) =>
	getJson<string[]>('/api/host-bridges', fetcher);

/** A new network. */
export type NewNetwork =
	| { mode: 'nat' | 'isolated'; name: string; subnet: string; autostart: boolean }
	| { mode: 'bridge'; name: string; bridge: string; autostart: boolean };

/** Creates and starts a network, and returns its UUID. */
export function createNetwork(
	network: NewNetwork,
	options: { csrf?: string; fetcher?: typeof fetch } = {}
): Promise<{ uuid: string }> {
	return send<{ uuid: string }>('POST', '/api/networks', { body: network, ...options });
}

/** Starts or stops a network, or switches its autostart. */
export function changeNetwork(
	id: string,
	change: { active?: boolean; autostart?: boolean },
	options: { csrf?: string; fetcher?: typeof fetch } = {}
): Promise<null> {
	return send<null>('PATCH', `/api/networks/${id}`, { body: change, ...options });
}

/** Deletes a network. `confirm` is its name as the user typed it. */
export function deleteNetwork(
	id: string,
	options: { confirm: string; csrf?: string; fetcher?: typeof fetch }
): Promise<null> {
	return send<null>('DELETE', `/api/networks/${id}`, {
		body: { confirm: options.confirm },
		csrf: options.csrf,
		fetcher: options.fetcher
	});
}

/** The state of a storage pool (`crates/lodger-core/src/model/pool.rs`). */
export type PoolState =
	'inactive' | 'building' | 'running' | 'degraded' | 'inaccessible' | 'unknown';

/** A storage pool, as `GET /api/pools` lists it. */
export interface Pool {
	uuid: string;
	name: string;
	state: PoolState;
	/** Sizes in bytes. libvirt reports 0 for an inactive pool. */
	capacity_bytes: number;
	allocation_bytes: number;
	available_bytes: number;
	persistent: boolean;
	autostart: boolean;
}

/** One pool with its type, folder, NFS source, and users (`crates/lodger/src/pools.rs`). */
export interface PoolDetail extends Pool {
	/** libvirt's pool type, such as `dir` or `netfs`. */
	kind: string | null;
	path: string | null;
	nfs?: { host: string; export: string };
	/** The VMs with a disk in the pool. */
	used_by: string[];
}

export const fetchPools = (fetcher?: typeof fetch) => getJson<Pool[]>('/api/pools', fetcher);
export const fetchPool = (id: string, fetcher?: typeof fetch) =>
	getJson<PoolDetail>(`/api/pools/${id}`, fetcher);

/** A new pool. `path` is required for `dir` and optional for `nfs`. */
export type NewPool =
	| { kind: 'dir'; name: string; path: string; autostart: boolean }
	| { kind: 'nfs'; name: string; host: string; export: string; path?: string; autostart: boolean };

/** Creates, builds, and starts a pool, and returns its UUID. */
export function createPool(
	pool: NewPool,
	options: { csrf?: string; fetcher?: typeof fetch } = {}
): Promise<{ uuid: string }> {
	return send<{ uuid: string }>('POST', '/api/pools', { body: pool, ...options });
}

/** Starts or stops a pool, or switches its autostart. */
export function changePool(
	id: string,
	change: { active?: boolean; autostart?: boolean },
	options: { csrf?: string; fetcher?: typeof fetch } = {}
): Promise<null> {
	return send<null>('PATCH', `/api/pools/${id}`, { body: change, ...options });
}

/**
 * Removes a pool. `confirm` is the pool's name as the user typed it. With
 * `deleteFiles`, libvirt also deletes every volume in it.
 */
export function removePool(
	id: string,
	options: { confirm: string; deleteFiles: boolean; csrf?: string; fetcher?: typeof fetch }
): Promise<null> {
	return send<null>('DELETE', `/api/pools/${id}`, {
		body: { confirm: options.confirm, delete_files: options.deleteFiles },
		csrf: options.csrf,
		fetcher: options.fetcher
	});
}

/** One volume of a pool (`crates/lodger-core/src/model/volume.rs`). */
export interface Volume {
	name: string;
	key: string;
	path: string;
	kind: string;
	/** The disk format, such as `qcow2` or `raw`. */
	format: string | null;
	/** The size that the guest sees. */
	capacity_bytes: number;
	/** The space that the volume takes on the host. */
	allocation_bytes: number;
	/** The VMs with a disk on this volume. */
	used_by: string[];
	/** The qcow2 overlays in the same pool that use this volume as their backing file. */
	backing_for: string[];
}

/** The volumes of a running pool, sorted by name. */
export const fetchVolumes = (poolId: string, fetcher?: typeof fetch) =>
	getJson<Volume[]>(`/api/pools/${poolId}/volumes`, fetcher);

/** A new volume. The server allows 1 MiB to 1 PiB. */
export interface NewVolume {
	name: string;
	format: 'qcow2' | 'raw';
	capacity_bytes: number;
}

/** Creates a volume in a running pool. */
export function createVolume(
	poolId: string,
	volume: NewVolume,
	options: { csrf?: string; fetcher?: typeof fetch } = {}
): Promise<{ name: string }> {
	return send<{ name: string }>('POST', `/api/pools/${poolId}/volumes`, {
		body: volume,
		...options
	});
}

/** Deletes a volume. The server refuses one that a VM uses, and names the VMs. */
export function deleteVolume(
	poolId: string,
	name: string,
	options: { csrf?: string; fetcher?: typeof fetch } = {}
): Promise<null> {
	return send<null>('DELETE', `/api/pools/${poolId}/volumes/${encodeURIComponent(name)}`, options);
}

/** A size in bytes, in the units of `formatKib`. */
export function formatBytes(bytes: number): string {
	return formatKib(bytes / 1024);
}

/**
 * Formats a rate in bytes per second, for example `1.5 MB/s`. It rounds before
 * it picks the unit, so 999,950 B/s shows as `1 MB/s`, not `1000 kB/s`.
 */
export function formatRate(bytesPerSecond: number): string {
	const units = ['B/s', 'kB/s', 'MB/s', 'GB/s'];
	const round = (v: number, unit: number) => (unit === 0 ? Math.round(v) : Number(v.toFixed(1)));
	let unit = 0;
	let rounded = round(bytesPerSecond, 0);
	while (rounded >= 1000 && unit < units.length - 1) {
		unit += 1;
		rounded = round(bytesPerSecond / 1000 ** unit, unit);
	}
	return `${rounded} ${units[unit]}`;
}

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
