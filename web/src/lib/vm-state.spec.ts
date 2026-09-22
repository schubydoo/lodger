import { describe, expect, it } from 'vitest';
import type { VmState } from './api';
import { stateLook } from './vm-state';

const states: VmState[] = [
	'no_state',
	'running',
	'blocked',
	'paused',
	'shutting_down',
	'shutoff',
	'crashed',
	'suspended',
	'unknown'
];

describe('stateLook', () => {
	it('gives every state a text label and an icon, not only a color', () => {
		for (const state of states) {
			const look = stateLook(state);
			expect(look.label.length).toBeGreaterThan(0);
			expect(look.icon.length).toBeGreaterThan(0);
		}
	});

	it('gives each state its own label', () => {
		const labels = new Set(states.map((s) => stateLook(s).label));
		expect(labels.size).toBe(states.length);
	});

	it('tells running and shut off apart by icon, not only by color', () => {
		expect(stateLook('running').icon).not.toBe(stateLook('shutoff').icon);
	});

	it('falls back to Unknown for a state from a newer server', () => {
		expect(stateLook('hibernating' as VmState).label).toBe('Unknown');
	});
});
