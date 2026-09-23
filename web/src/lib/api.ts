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
	session: ['session'] as const,
	setup: ['setup'] as const,
	accounts: ['accounts'] as const
};

/** An error answer, with its status so the app can react to a 401. */
export class ApiError extends Error {
	constructor(
		readonly status: number,
		message: string
	) {
		super(message);
	}
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
	method: 'POST' | 'DELETE',
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
			typeof message === 'string' ? message : `${path} answered ${res.status} ${res.statusText}`
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
export type VmAction = 'start' | 'shutdown' | 'force-off';

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
