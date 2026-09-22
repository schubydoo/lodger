import { describe, expect, it } from 'vitest';
import { statusText, vncUrl } from './console';

describe('vncUrl', () => {
	it('uses wss behind TLS and ws otherwise', () => {
		const id = '00000000-0000-0000-0000-000000000001';
		expect(vncUrl({ protocol: 'https:', host: 'lodger.lan' }, id)).toBe(
			`wss://lodger.lan/ws/vms/${id}/vnc`
		);
		expect(vncUrl({ protocol: 'http:', host: '127.0.0.1:8460' }, id)).toBe(
			`ws://127.0.0.1:8460/ws/vms/${id}/vnc`
		);
	});

	it('escapes the id', () => {
		expect(vncUrl({ protocol: 'http:', host: 'h' }, 'a/b')).toBe('ws://h/ws/vms/a%2Fb/vnc');
	});
});

describe('statusText', () => {
	it('has a sentence for each status', () => {
		for (const s of ['connecting', 'connected', 'closed', 'failed'] as const) {
			expect(statusText(s).length).toBeGreaterThan(0);
		}
	});
});
