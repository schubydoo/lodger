import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { flushSync } from 'svelte';
import Page from './+page.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { testClient, vms } from '$lib/test/fixtures';
import { keys } from '$lib/api';

// A stand-in for noVNC's RFB class that records what the page does.
const rfb = vi.hoisted(() => ({
	instances: [] as {
		url: string;
		options: unknown;
		disconnected: boolean;
		ctrlAltDel: number;
		fire: (type: string, detail?: unknown) => void;
	}[]
}));

vi.mock('@novnc/novnc', () => ({
	default: class extends EventTarget {
		scaleViewport = false;
		background = '';
		state: (typeof rfb.instances)[number];
		constructor(_target: HTMLElement, url: string, options: unknown) {
			super();
			this.state = {
				url,
				options,
				disconnected: false,
				ctrlAltDel: 0,
				fire: (type, detail) => this.dispatchEvent(new CustomEvent(type, { detail }))
			};
			rfb.instances.push(this.state);
		}
		disconnect() {
			this.state.disconnected = true;
		}
		sendCtrlAltDel() {
			this.state.ctrlAltDel += 1;
		}
	}
}));

const params = vi.hoisted(() => ({ name: 'alpha' }));
vi.mock('$app/state', () => ({ page: { params } }));

function show(name: string, data: typeof vms | null = vms) {
	params.name = name;
	const client = testClient();
	if (data) client.setQueryData(keys.vms, data);
	return { client, ...render(QueryHarness, { props: { client, component: Page, props: {} } }) };
}

beforeEach(() => (rfb.instances = []));
afterEach(() => vi.clearAllMocks());

describe('the console page', () => {
	it('connects noVNC to the VM socket and reports the status', async () => {
		show('alpha');
		expect(rfb.instances).toHaveLength(1);
		expect(rfb.instances[0].url).toBe(`ws://${window.location.host}/ws/vms/${vms[0].uuid}/vnc`);
		expect(screen.getByRole('status')).toHaveTextContent('Connecting');
		const button = screen.getByRole('button', { name: 'Send Ctrl+Alt+Del' });
		expect(button).toBeDisabled();

		rfb.instances[0].fire('connect');
		expect(await screen.findByText(/Connected/)).toBeInTheDocument();
		await fireEvent.click(button);
		expect(rfb.instances[0].ctrlAltDel).toBe(1);

		rfb.instances[0].fire('disconnect', { clean: false });
		expect(await screen.findByText(/could not connect or closed/)).toBeInTheDocument();
	});

	it('disconnects when the page goes away', () => {
		const { unmount } = show('alpha');
		unmount();
		expect(rfb.instances[0].disconnected).toBe(true);
	});

	it('keeps the console when another field of the VM changes', () => {
		const { client } = show('alpha');
		expect(rfb.instances).toHaveLength(1);
		// For example `virsh setmem`: the list refetches with new memory.
		client.setQueryData(keys.vms, [{ ...vms[0], memory_kib: 8388608 }, vms[1]]);
		flushSync();
		expect(rfb.instances).toHaveLength(1);
		expect(rfb.instances[0].disconnected).toBe(false);
	});

	it('disconnects when the VM stops', () => {
		const { client } = show('alpha');
		client.setQueryData(keys.vms, [{ ...vms[0], state: 'shutoff' }, vms[1]]);
		flushSync();
		expect(rfb.instances[0].disconnected).toBe(true);
		expect(screen.getByText(/is not running/)).toBeInTheDocument();
	});

	it('shows loading, then an error answer from the API', async () => {
		vi.stubGlobal(
			'fetch',
			vi.fn(async () => new Response('', { status: 503, statusText: 'Service Unavailable' }))
		);
		show('alpha', null);
		expect(screen.getByText('Loading…')).toBeInTheDocument();
		expect(await screen.findByRole('alert')).toHaveTextContent('/api/vms answered 503');
		vi.unstubAllGlobals();
	});

	it('does not connect to a VM that is not running', () => {
		show('beta');
		expect(screen.getByText(/is not running/)).toBeInTheDocument();
		expect(rfb.instances).toHaveLength(0);
	});

	it('says so when no VM has the name', () => {
		show('gamma');
		expect(screen.getByRole('alert')).toHaveTextContent('No virtual machine is called gamma');
		expect(rfb.instances).toHaveLength(0);
	});
});
