import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import StateBadge from './StateBadge.svelte';
import type { VmState } from '$lib/api';

describe('StateBadge', () => {
	it('shows the state as text and an icon that screen readers skip', () => {
		const { container } = render(StateBadge, { state: 'running' });
		expect(screen.getByText('Running')).toBeInTheDocument();
		const icon = container.querySelector('svg');
		expect(icon).not.toBeNull();
		expect(icon).toHaveAttribute('aria-hidden', 'true');
	});

	it('uses a different icon for running and shut off', () => {
		const svg = (state: VmState) =>
			render(StateBadge, { state }).container.querySelector('svg')?.innerHTML;
		expect(svg('running')).not.toBe(svg('shutoff'));
	});

	it('shows Unknown for a state it does not know', () => {
		render(StateBadge, { state: 'hibernating' as VmState });
		expect(screen.getByText('Unknown')).toBeInTheDocument();
	});
});
