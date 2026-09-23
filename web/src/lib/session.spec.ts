import { describe, expect, it, vi } from 'vitest';
import { ticketSource, withTicket } from './session';

const json = (body: unknown, status = 200) =>
	new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });

describe('ticketSource', () => {
	it('sends the CSRF token and returns the ticket', async () => {
		const fetcher = vi.fn(async (path: RequestInfo | URL) =>
			path === '/api/session' ? json({ csrf_token: 'c1' }) : json({ ticket: 't1' }, 201)
		);
		const next = ticketSource(fetcher as typeof fetch);
		await expect(next()).resolves.toBe('t1');
		expect(fetcher).toHaveBeenLastCalledWith('/api/ws-tickets', {
			method: 'POST',
			headers: { 'x-csrf-token': 'c1' }
		});
	});

	it('reads the current CSRF token for each ticket, so a new login needs no retry', async () => {
		const tokens = ['first', 'second'];
		const sent: string[] = [];
		const fetcher = vi.fn(async (path: RequestInfo | URL, init?: RequestInit) => {
			if (path === '/api/session') return json({ csrf_token: tokens.shift() });
			sent.push((init?.headers as Record<string, string>)['x-csrf-token']);
			return json({ ticket: 't' }, 201);
		});
		const next = ticketSource(fetcher as typeof fetch);
		await next();
		await next();
		expect(sent).toEqual(['first', 'second']);
	});

	it('throws with the status when there is no session', async () => {
		const fetcher = vi.fn(
			async () => new Response('', { status: 401, statusText: 'Unauthorized' })
		);
		await expect(ticketSource(fetcher as typeof fetch)()).rejects.toThrow(
			'/api/session answered 401'
		);
	});

	it('throws with the status when the ticket is refused', async () => {
		const fetcher = vi.fn(async (path: RequestInfo | URL) =>
			path === '/api/session' ? json({ csrf_token: 'c' }) : json({}, 403)
		);
		await expect(ticketSource(fetcher as typeof fetch)()).rejects.toThrow(
			'/api/ws-tickets answered 403'
		);
	});
});

describe('withTicket', () => {
	it('puts the escaped ticket in the query', () => {
		expect(withTicket('ws://h/ws/events', 'a b')).toBe('ws://h/ws/events?ticket=a%20b');
	});
});
