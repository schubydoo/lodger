// How each VM state shows in the UI. WCAG 2.2 AA and PRD 5.4: state shows
// as text and an icon, never as color alone. The color is only an extra.

import type { VmState } from './api';

export type StateIcon = 'play' | 'pause' | 'square' | 'loader' | 'alert' | 'moon' | 'help';

export interface StateLook {
	label: string;
	icon: StateIcon;
	/** Tailwind classes for the badge. The label carries the meaning. */
	tone: string;
}

const looks: Record<VmState, StateLook> = {
	running: { label: 'Running', icon: 'play', tone: 'text-green-800 dark:text-green-300' },
	blocked: { label: 'Blocked', icon: 'alert', tone: 'text-amber-800 dark:text-amber-300' },
	paused: { label: 'Paused', icon: 'pause', tone: 'text-amber-800 dark:text-amber-300' },
	shutting_down: {
		label: 'Shutting down',
		icon: 'loader',
		tone: 'text-amber-800 dark:text-amber-300'
	},
	shutoff: { label: 'Shut off', icon: 'square', tone: 'text-muted-foreground' },
	crashed: { label: 'Crashed', icon: 'alert', tone: 'text-red-800 dark:text-red-300' },
	suspended: { label: 'Suspended', icon: 'moon', tone: 'text-muted-foreground' },
	no_state: { label: 'No state', icon: 'help', tone: 'text-muted-foreground' },
	unknown: { label: 'Unknown', icon: 'help', tone: 'text-muted-foreground' }
};

export function stateLook(state: VmState): StateLook {
	return looks[state] ?? looks.unknown;
}
