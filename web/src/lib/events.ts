// The client for /ws/events (crates/lodger/src/ws.rs). The server sends small
// JSON messages that name what changed. The client does not apply them to any
// data itself: it tells TanStack Query which queries to fetch again.

import type { QueryClient } from '@tanstack/svelte-query';
import { keys, type Connection } from './api';

export type Update =
	| { type: 'vm'; id: string }
	| { type: 'pool'; id: string }
	| { type: 'network'; id: string }
	| { type: 'resync' }
	| ({ type: 'connection' } & Connection);

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
	return ['vm', 'pool', 'network', 'resync', 'connection'].includes(type as string)
		? (value as Update)
		: null;
}

/** Waits between reconnect attempts: 1 s, 2 s, 4 s, and then at most 5 s. */
export function retryDelay(attempt: number): number {
	return Math.min(1000 * 2 ** attempt, 5000);
}

export interface EventSocketOptions {
	url: string;
	client: QueryClient;
	/** Called with `true` when the socket opens and `false` when it closes. */
	onOpenChange?: (open: boolean) => void;
	/** For tests. */
	createSocket?: (url: string) => WebSocket;
	/** For tests. */
	setTimer?: (fn: () => void, ms: number) => unknown;
}

/**
 * Keeps one WebSocket open and reconnects after a close. After each
 * reconnect it fetches every query again, because events may have been
 * missed while the socket was closed.
 */
export function connectEvents(options: EventSocketOptions): () => void {
	const create = options.createSocket ?? ((url: string) => new WebSocket(url));
	const setTimer = options.setTimer ?? ((fn: () => void, ms: number) => setTimeout(fn, ms));
	let attempt = 0;
	let stopped = false;
	let socket: WebSocket | null = null;
	let opened = false;

	const open = () => {
		socket = create(options.url);
		socket.onopen = () => {
			if (opened) void options.client.invalidateQueries();
			opened = true;
			attempt = 0;
			options.onOpenChange?.(true);
		};
		socket.onmessage = (event: MessageEvent) => {
			const update = parseUpdate(String(event.data));
			if (!update) return;
			const stale = invalidationsFor(update);
			if (stale === 'all') {
				void options.client.invalidateQueries();
				return;
			}
			for (const queryKey of stale) {
				void options.client.invalidateQueries({ queryKey: [...queryKey] });
			}
		};
		socket.onclose = () => {
			options.onOpenChange?.(false);
			if (stopped) return;
			setTimer(open, retryDelay(attempt));
			attempt += 1;
		};
	};

	open();
	return () => {
		stopped = true;
		socket?.close();
	};
}

/** The events URL for the page's own origin. */
export function eventsUrl(location: Location): string {
	const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
	return `${scheme}//${location.host}/ws/events`;
}
