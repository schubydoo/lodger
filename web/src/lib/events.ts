// The client for /ws/events (crates/lodger/src/ws.rs). The server sends small
// JSON messages that name what changed. The client does not apply them to any
// data itself: it tells TanStack Query which queries to fetch again. The one
// exception is `stats`, which carries the data and goes straight into the
// query cache. Only a page that calls `wantStats` makes the server send it.

import type { QueryClient } from '@tanstack/svelte-query';
import { keys, type Connection, type VmStats } from './api';

export type Update =
	| { type: 'vm'; id: string }
	| { type: 'pool'; id: string }
	| { type: 'network'; id: string }
	| { type: 'resync' }
	| ({ type: 'connection' } & Connection)
	| { type: 'stats'; vms: VmStats[] };

const SUBSCRIBE = JSON.stringify({ subscribe: 'stats' });
const UNSUBSCRIBE = JSON.stringify({ unsubscribe: 'stats' });

/** The pages that want live stats, and the open socket that asks for them. */
const statsInterest: {
	count: number;
	send: ((text: string) => void) | null;
	client: QueryClient | null;
} = { count: 0, send: null, client: null };

/**
 * Asks the server for live stats while at least one page wants them. Call it
 * when a page opens, and call the returned function when it closes. The
 * server polls libvirt only while some socket subscribes (TAD 6.2).
 */
export function wantStats(): () => void {
	statsInterest.count += 1;
	if (statsInterest.count === 1) statsInterest.send?.(SUBSCRIBE);
	let released = false;
	return () => {
		if (released) return;
		released = true;
		statsInterest.count -= 1;
		if (statsInterest.count === 0) {
			statsInterest.send?.(UNSUBSCRIBE);
			// Old values must not show as live when a page opens later.
			statsInterest.client?.removeQueries({ queryKey: [...keys.stats] });
		}
	};
}

/** The query keys to fetch again after `update`, or `'all'` for every query. */
export function invalidationsFor(update: Update): (readonly unknown[])[] | 'all' {
	switch (update.type) {
		// A VM change can move the host's VM counts too.
		case 'vm':
			return [keys.vms, keys.host];
		case 'pool':
		case 'network':
			return [keys.host];
		case 'connection':
			return [keys.host];
		case 'resync':
			return 'all';
		case 'stats':
			return [];
	}
}

/** Parses one message. Returns `null` for anything that is not a known update. */
export function parseUpdate(text: string): Update | null {
	let value: unknown;
	try {
		value = JSON.parse(text);
	} catch {
		return null;
	}
	if (typeof value !== 'object' || value === null) return null;
	const type = (value as { type?: unknown }).type;
	return ['vm', 'pool', 'network', 'resync', 'connection', 'stats'].includes(type as string)
		? (value as Update)
		: null;
}

/** Waits between reconnect attempts: 1 s, 2 s, 4 s, and then at most 5 s. */
export function retryDelay(attempt: number): number {
	return Math.min(1000 * 2 ** attempt, 5000);
}

export interface EventSocketOptions {
	/** The URL for the next connection. Each one needs a fresh ticket. */
	url: () => Promise<string>;
	client: QueryClient;
	/** Called with `true` when the socket opens and `false` when it closes. */
	onOpenChange?: (open: boolean) => void;
	/** For tests. */
	createSocket?: (url: string) => WebSocket;
	/** For tests. */
	setTimer?: (fn: () => void, ms: number) => unknown;
}

/**
 * Keeps one WebSocket open and reconnects after a close. Each time the
 * socket opens, the first time too, it fetches every query again. Pages
 * fetch before the socket opens, and events may have been
 * missed while the socket was closed.
 */
export function connectEvents(options: EventSocketOptions): () => void {
	const create = options.createSocket ?? ((url: string) => new WebSocket(url));
	const setTimer = options.setTimer ?? ((fn: () => void, ms: number) => setTimeout(fn, ms));
	let attempt = 0;
	let stopped = false;
	let socket: WebSocket | null = null;

	const retry = () => {
		if (stopped) return;
		setTimer(open, retryDelay(attempt));
		attempt += 1;
	};

	const open = () => {
		options.url().then(connect, () => {
			// No ticket, for example after a logout: try again later.
			options.onOpenChange?.(false);
			retry();
		});
	};

	const connect = (url: string) => {
		if (stopped) return;
		const current = create(url);
		socket = current;
		current.onopen = () => {
			void options.client.invalidateQueries();
			attempt = 0;
			statsInterest.client = options.client;
			statsInterest.send = (text) => current.send(text);
			// A new socket has no subscription yet, the first one or not.
			if (statsInterest.count > 0) current.send(SUBSCRIBE);
			options.onOpenChange?.(true);
		};
		current.onmessage = (event: MessageEvent) => {
			const update = parseUpdate(String(event.data));
			if (!update) return;
			if (update.type === 'stats') {
				options.client.setQueryData(keys.stats, update.vms);
				return;
			}
			const stale = invalidationsFor(update);
			if (stale === 'all') {
				void options.client.invalidateQueries();
				return;
			}
			for (const queryKey of stale) {
				void options.client.invalidateQueries({ queryKey: [...queryKey] });
			}
		};
		current.onclose = () => {
			if (socket === current) statsInterest.send = null;
			options.onOpenChange?.(false);
			retry();
		};
	};

	open();
	return () => {
		stopped = true;
		statsInterest.send = null;
		socket?.close();
	};
}

/** The events URL for the page's own origin. */
export function eventsUrl(location: Location): string {
	const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
	return `${scheme}//${location.host}/ws/events`;
}
