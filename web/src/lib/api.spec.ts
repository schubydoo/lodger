import { describe, expect, it, vi } from 'vitest';
import { fetchVms, formatKib, keys } from './api';

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
