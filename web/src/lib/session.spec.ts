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

	it('keeps the CSRF token between tickets', async () => {
		const fetcher = vi.fn(async (path: RequestInfo | URL) =>
			path === '/api/session' ? json({ csrf_token: 'c1' }) : json({ ticket: 't' }, 201)
		);
		const next = ticketSource(fetcher as typeof fetch);
		await next();
		await next();
		expect(fetcher.mock.calls.filter(([p]) => p === '/api/session')).toHaveLength(1);
	});

	it('fetches the token again once after a 403', async () => {
		const tokens = ['old', 'new'];
		const fetcher = vi.fn(async (path: RequestInfo | URL, init?: RequestInit) => {
			if (path === '/api/session') return json({ csrf_token: tokens.shift() });
			const sent = (init?.headers as Record<string, string>)['x-csrf-token'];
			return sent === 'new' ? json({ ticket: 't2' }, 201) : json({}, 403);
		});
		await expect(ticketSource(fetcher as typeof fetch)()).resolves.toBe('t2');
	});

	it('throws with the status when there is no session', async () => {
		const fetcher = vi.fn(
			async () => new Response('', { status: 401, statusText: 'Unauthorized' })
		);
		await expect(ticketSource(fetcher as typeof fetch)()).rejects.toThrow(
			'/api/session answered 401'
		);
	});

	it('throws when the token is still refused after the retry', async () => {
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
