import { describe, expect, it, vi } from 'vitest';
import type { QueryClient } from '@tanstack/svelte-query';
import { connectEvents, eventsUrl, invalidationsFor, parseUpdate, retryDelay } from './events';

const id = '00000000-0000-0000-0000-000000000007';

describe('invalidationsFor', () => {
	it('refetches the VM list and the host counts after a VM change', () => {
		expect(invalidationsFor({ type: 'vm', id })).toEqual([['vms'], ['host']]);
	});

	it('refetches the host after a pool, network, or connection change', () => {
		expect(invalidationsFor({ type: 'pool', id })).toEqual([['host']]);
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
	close() {
		this.closed = true;
		this.onclose?.();
	}
	receive(data: string) {
		this.onmessage?.({ data } as MessageEvent);
	}
}

function setup() {
	const sockets: FakeSocket[] = [];
	const timers: { fn: () => void; ms: number }[] = [];
	const invalidateQueries = vi.fn(async () => {});
	const opens: boolean[] = [];
	const stop = connectEvents({
		url: 'ws://test/ws/events',
		client: { invalidateQueries } as unknown as QueryClient,
		onOpenChange: (open) => opens.push(open),
		createSocket: () => {
			const s = new FakeSocket();
			sockets.push(s);
			return s as unknown as WebSocket;
		},
		setTimer: (fn, ms) => timers.push({ fn, ms })
	});
	return { sockets, timers, invalidateQueries, opens, stop };
}

describe('connectEvents', () => {
	it('invalidates the queries that a message names', () => {
		const t = setup();
		t.sockets[0].onopen?.();
		t.sockets[0].receive(`{"type":"vm","id":"${id}"}`);
		expect(t.invalidateQueries).toHaveBeenCalledWith({ queryKey: ['vms'] });
		expect(t.invalidateQueries).toHaveBeenCalledWith({ queryKey: ['host'] });
	});

	it('invalidates every query on resync', () => {
		const t = setup();
		t.sockets[0].onopen?.();
		t.sockets[0].receive('{"type":"resync"}');
		expect(t.invalidateQueries).toHaveBeenCalledWith();
	});

	it('reconnects after a close and then refetches everything', () => {
		const t = setup();
		t.sockets[0].onopen?.();
		expect(t.invalidateQueries).not.toHaveBeenCalled();

		t.sockets[0].onclose?.();
		expect(t.opens).toEqual([true, false]);
		expect(t.timers).toEqual([{ fn: expect.any(Function), ms: 1000 }]);

		t.timers[0].fn();
		t.sockets[1].onopen?.();
		expect(t.opens).toEqual([true, false, true]);
		// Events may have been missed while the socket was closed.
		expect(t.invalidateQueries).toHaveBeenCalledWith();
	});

	it('backs off while the server stays away', () => {
		const t = setup();
		t.sockets[0].onclose?.();
		t.timers[0].fn();
		t.sockets[1].onclose?.();
		expect(t.timers.map((x) => x.ms)).toEqual([1000, 2000]);
	});

	it('stops reconnecting after stop()', () => {
		const t = setup();
		t.stop();
		expect(t.sockets[0].closed).toBe(true);
		expect(t.timers).toEqual([]);
	});
});
