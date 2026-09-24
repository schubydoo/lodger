import { describe, expect, it, vi } from 'vitest';
import {
	ApiError,
	fetchSession,
	fetchSetupOpen,
	fetchVms,
	formatKib,
	formatRate,
	keys,
	problemText,
	send
} from './api';

describe('formatRate', () => {
	it('uses decimal units per second', () => {
		expect(formatRate(0)).toBe('0 B/s');
		expect(formatRate(999)).toBe('999 B/s');
		expect(formatRate(1500)).toBe('1.5 kB/s');
		expect(formatRate(1_000_000)).toBe('1 MB/s');
		expect(formatRate(2_345_000_000)).toBe('2.3 GB/s');
	});

	it('moves up a unit when the rounding reaches 1000', () => {
		expect(formatRate(999.6)).toBe('1 kB/s');
		expect(formatRate(999_950)).toBe('1 MB/s');
		expect(formatRate(999_949)).toBe('999.9 kB/s');
		expect(formatRate(999_960_000)).toBe('1 GB/s');
		expect(formatRate(5_000_000_000_000)).toBe('5000 GB/s');
	});
});

describe('formatKib', () => {
	it('uses binary units', () => {
		expect(formatKib(512)).toBe('512 KiB');
		expect(formatKib(65536)).toBe('64 MiB');
		expect(formatKib(4194304)).toBe('4 GiB');
		expect(formatKib(3145728 * 1024)).toBe('3 TiB');
	});

	it('keeps one decimal for a fraction', () => {
		expect(formatKib(1536)).toBe('1.5 MiB');
		expect(formatKib(65610412)).toBe('62.6 GiB');
	});
});

describe('fetchVms', () => {
	it('returns the JSON body', async () => {
		const fetcher = vi.fn(async () => new Response('[]', { status: 200 }));
		await expect(fetchVms(fetcher)).resolves.toEqual([]);
		expect(fetcher).toHaveBeenCalledWith('/api/vms', expect.anything());
	});

	it('throws with the status on an error answer', async () => {
		const fetcher = vi.fn(
			async () => new Response('', { status: 503, statusText: 'Service Unavailable' })
		);
		await expect(fetchVms(fetcher)).rejects.toThrow('/api/vms answered 503');
	});
});

describe('keys', () => {
	it('nests one VM under the list, so a list change also covers it', () => {
		expect(keys.vm('abc').slice(0, 1)).toEqual([...keys.vms]);
	});
});

describe('send', () => {
	it('sends JSON with the CSRF token and returns the answer', async () => {
		const fetcher = vi.fn(async () => new Response('{"id":3}', { status: 201 }));
		await expect(
			send('POST', '/api/accounts', { body: { username: 'x' }, csrf: 'c1', fetcher })
		).resolves.toEqual({ id: 3 });
		expect(fetcher).toHaveBeenCalledWith('/api/accounts', {
			method: 'POST',
			headers: {
				accept: 'application/json',
				'content-type': 'application/json',
				'x-csrf-token': 'c1'
			},
			body: '{"username":"x"}'
		});
	});

	it('sends no body and no content type without a body, and accepts an empty answer', async () => {
		const fetcher = vi.fn(async () => new Response(null, { status: 204 }));
		await expect(send('DELETE', '/api/session', { fetcher })).resolves.toBeNull();
		expect(fetcher).toHaveBeenCalledWith('/api/session', {
			method: 'DELETE',
			headers: { accept: 'application/json' },
			body: undefined
		});
	});

	it("throws the server's message with the status", async () => {
		const fetcher = vi.fn(async () => new Response('{"error":"no such account"}', { status: 404 }));
		const error = (await send('DELETE', '/api/accounts/9', { fetcher }).catch(
			(e) => e
		)) as ApiError;
		expect(error).toBeInstanceOf(ApiError);
		expect(error.status).toBe(404);
		expect(error.message).toBe('no such account');
	});

	it('falls back to the status for an answer that is not JSON', async () => {
		const fetcher = vi.fn(
			async () =>
				new Response('<html>Bad Gateway</html>', { status: 502, statusText: 'Bad Gateway' })
		);
		await expect(send('POST', '/api/session', { fetcher })).rejects.toThrow(
			'/api/session answered 502 Bad Gateway'
		);
	});
});

describe('fetchSession and fetchSetupOpen', () => {
	it('turn the expected error answers into values', async () => {
		const status = (code: number, body = '{}') =>
			vi.fn(async () => new Response(body, { status: code }));
		await expect(fetchSession(status(401))).resolves.toBeNull();
		await expect(fetchSession(status(200, '{"username":"a","csrf_token":"c"}'))).resolves.toEqual({
			username: 'a',
			csrf_token: 'c'
		});
		await expect(fetchSession(status(500))).rejects.toThrow('/api/session answered 500');
		await expect(fetchSetupOpen(status(200))).resolves.toBe(true);
		await expect(fetchSetupOpen(status(404))).resolves.toBe(false);
		await expect(fetchSetupOpen(status(503))).rejects.toThrow('/api/setup answered 503');
	});
});

describe('problemText', () => {
	it('makes a sentence of a server message', () => {
		expect(problemText(new Error('wrong setup token'))).toBe('Wrong setup token.');
		expect(problemText(new Error('Done already.'))).toBe('Done already.');
		expect(problemText('')).toBe('Something went wrong.');
	});
});
