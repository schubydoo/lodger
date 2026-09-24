import { describe, expect, it, vi } from 'vitest';
import type { QueryClient } from '@tanstack/svelte-query';
import {
	connectEvents,
	eventsUrl,
	invalidationsFor,
	parseUpdate,
	retryDelay,
	wantStats
} from './events';

const id = '00000000-0000-0000-0000-000000000007';

describe('invalidationsFor', () => {
	it('refetches the VM list and the host counts after a VM change', () => {
		expect(invalidationsFor({ type: 'vm', id })).toEqual([['vms'], ['host']]);
	});

	it('refetches the host after a pool, network, or connection change', () => {
		expect(invalidationsFor({ type: 'pool', id })).toEqual([['pools'], ['host']]);
		expect(invalidationsFor({ type: 'network', id })).toEqual([['host']]);
		expect(invalidationsFor({ type: 'connection', state: 'connected' })).toEqual([['host']]);
	});

	it('refetches everything after a resync', () => {
		expect(invalidationsFor({ type: 'resync' })).toBe('all');
	});
});

describe('parseUpdate', () => {
	it('reads the server messages', () => {
		expect(parseUpdate(`{"type":"vm","id":"${id}"}`)).toEqual({ type: 'vm', id });
		expect(parseUpdate('{"type":"resync"}')).toEqual({ type: 'resync' });
	});

	it('ignores anything else', () => {
		expect(parseUpdate('not json')).toBeNull();
		expect(parseUpdate('null')).toBeNull();
		expect(parseUpdate('{"type":"reboot"}')).toBeNull();
	});
});

describe('retryDelay', () => {
	it('backs off to at most 5 seconds', () => {
		expect([0, 1, 2, 3, 10].map(retryDelay)).toEqual([1000, 2000, 4000, 5000, 5000]);
	});
});

describe('eventsUrl', () => {
	it('uses wss behind TLS and ws otherwise', () => {
		expect(eventsUrl({ protocol: 'https:', host: 'lodger.lan' } as Location)).toBe(
			'wss://lodger.lan/ws/events'
		);
		expect(eventsUrl({ protocol: 'http:', host: '127.0.0.1:8460' } as Location)).toBe(
			'ws://127.0.0.1:8460/ws/events'
		);
	});
});

/** A stand-in for the browser WebSocket that the test drives by hand. */
class FakeSocket {
	onopen: (() => void) | null = null;
	onmessage: ((e: MessageEvent) => void) | null = null;
	onclose: (() => void) | null = null;
	closed = false;
	sent: string[] = [];
	send(text: string) {
		this.sent.push(text);
	}
	close() {
		this.closed = true;
		this.onclose?.();
	}
	receive(data: string) {
		this.onmessage?.({ data } as MessageEvent);
	}
}

/** Lets the pending promises, such as the URL, settle. */
const settle = () => new Promise((resolve) => setTimeout(resolve));

async function setup(url: () => Promise<string> = async () => 'ws://test/ws/events?ticket=t') {
	const sockets: FakeSocket[] = [];
	const urls: string[] = [];
	const timers: { fn: () => void; ms: number }[] = [];
	const invalidateQueries = vi.fn(async () => {});
	const setQueryData = vi.fn();
	const removeQueries = vi.fn();
	const opens: boolean[] = [];
	const stop = connectEvents({
		url,
		client: { invalidateQueries, setQueryData, removeQueries } as unknown as QueryClient,
		onOpenChange: (open) => opens.push(open),
		createSocket: (u) => {
			urls.push(u);
			const s = new FakeSocket();
			sockets.push(s);
			return s as unknown as WebSocket;
		},
		setTimer: (fn, ms) => timers.push({ fn, ms })
	});
	await settle();
	return { sockets, urls, timers, invalidateQueries, setQueryData, removeQueries, opens, stop };
}

const SUBSCRIBE = '{"subscribe":"stats"}';
const UNSUBSCRIBE = '{"unsubscribe":"stats"}';

describe('live stats', () => {
	it('puts the stats into the query cache, and invalidates nothing', async () => {
		const t = await setup();
		t.sockets[0].onopen?.();
		t.invalidateQueries.mockClear();
		const vms = [{ uuid: id, cpu_percent: 12.5 }];
		t.sockets[0].receive(JSON.stringify({ type: 'stats', vms }));
		expect(t.setQueryData).toHaveBeenCalledWith(['stats'], vms);
		expect(t.invalidateQueries).not.toHaveBeenCalled();
		t.stop();
	});

	it('subscribes while at least one page wants stats', async () => {
		const t = await setup();
		t.sockets[0].onopen?.();
		const first = wantStats();
		const second = wantStats();
		expect(t.sockets[0].sent).toEqual([SUBSCRIBE]);
		first();
		first();
		expect(t.sockets[0].sent).toEqual([SUBSCRIBE]);
		second();
		expect(t.sockets[0].sent).toEqual([SUBSCRIBE, UNSUBSCRIBE]);
		expect(t.removeQueries).toHaveBeenCalledWith({ queryKey: ['stats'] });
		t.stop();
	});

	it('subscribes when the socket opens, also after a reconnect', async () => {
		const t = await setup();
		const release = wantStats();
		// No open socket yet: nothing goes out, and nothing fails.
		expect(t.sockets[0].sent).toEqual([]);
		t.sockets[0].onopen?.();
		expect(t.sockets[0].sent).toEqual([SUBSCRIBE]);
		t.sockets[0].onclose?.();
		t.timers[0].fn();
		await settle();
		t.sockets[1].onopen?.();
		expect(t.sockets[1].sent).toEqual([SUBSCRIBE]);
		release();
		expect(t.sockets[1].sent).toEqual([SUBSCRIBE, UNSUBSCRIBE]);
		t.stop();
	});

	it('sends nothing to a closed socket', async () => {
		const t = await setup();
		t.sockets[0].onopen?.();
		t.sockets[0].onclose?.();
		const release = wantStats();
		release();
		expect(t.sockets[0].sent).toEqual([]);
		t.stop();
	});
});

describe('connectEvents', () => {
	it('invalidates the queries that a message names', async () => {
		const t = await setup();
		t.sockets[0].onopen?.();
		t.sockets[0].receive(`{"type":"vm","id":"${id}"}`);
		expect(t.invalidateQueries).toHaveBeenCalledWith({ queryKey: ['vms'] });
		expect(t.invalidateQueries).toHaveBeenCalledWith({ queryKey: ['host'] });
	});

	it('invalidates every query on resync', async () => {
		const t = await setup();
		t.sockets[0].onopen?.();
		t.sockets[0].receive('{"type":"resync"}');
		expect(t.invalidateQueries).toHaveBeenCalledWith();
	});

	it('refetches everything when the socket first opens', async () => {
		const t = await setup();
		expect(t.invalidateQueries).not.toHaveBeenCalled();
		// The page fetched before the socket opened. A change in between
		// sends no event, so the open itself must refetch.
		t.sockets[0].onopen?.();
		expect(t.invalidateQueries).toHaveBeenCalledWith();
	});

	it('reconnects after a close and then refetches everything', async () => {
		const t = await setup();
		t.sockets[0].onopen?.();
		t.invalidateQueries.mockClear();

		t.sockets[0].onclose?.();
		expect(t.opens).toEqual([true, false]);
		expect(t.timers).toEqual([{ fn: expect.any(Function), ms: 1000 }]);

		t.timers[0].fn();
		await settle();
		t.sockets[1].onopen?.();
		expect(t.opens).toEqual([true, false, true]);
		// Events may have been missed while the socket was closed.
		expect(t.invalidateQueries).toHaveBeenCalledWith();
	});

	it('backs off while the server stays away', async () => {
		const t = await setup();
		t.sockets[0].onclose?.();
		t.timers[0].fn();
		await settle();
		t.sockets[1].onclose?.();
		expect(t.timers.map((x) => x.ms)).toEqual([1000, 2000]);
	});

	it('asks for a new URL, and so a new ticket, for each connection', async () => {
		let n = 0;
		const t = await setup(async () => `ws://test/ws/events?ticket=${++n}`);
		t.sockets[0].onclose?.();
		t.timers[0].fn();
		await settle();
		expect(t.urls).toEqual(['ws://test/ws/events?ticket=1', 'ws://test/ws/events?ticket=2']);
	});

	it('retries later when no ticket comes, as after a logout', async () => {
		const t = await setup(async () => {
			throw new Error('/api/ws-tickets answered 401');
		});
		expect(t.sockets).toEqual([]);
		expect(t.opens).toEqual([false]);
		expect(t.timers.map((x) => x.ms)).toEqual([1000]);
	});

	it('opens no socket when stop() comes before the ticket', async () => {
		let give: (url: string) => void = () => {};
		const t = await setup(() => new Promise((resolve) => (give = resolve)));
		t.stop();
		give('ws://test/ws/events?ticket=late');
		await settle();
		expect(t.sockets).toEqual([]);
	});

	it('stops reconnecting after stop()', async () => {
		const t = await setup();
		t.stop();
		expect(t.sockets[0].closed).toBe(true);
		expect(t.timers).toEqual([]);
	});
});
