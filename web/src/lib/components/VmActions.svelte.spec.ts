import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import VmActions from './VmActions.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { testClient, vms } from '$lib/test/fixtures';
import { keys, type Vm } from '$lib/api';

const [running, shutoff] = vms;

function show(vm: Vm, respond: () => Response = () => new Response(null, { status: 204 })) {
	const fetcher = vi.fn<(path: string, init?: RequestInit) => Promise<Response>>(async () =>
		respond()
	);
	vi.stubGlobal('fetch', fetcher);
	const client = testClient();
	client.setQueryData(keys.session, { username: 'admin', csrf_token: 'csrf1' });
	const view = render(QueryHarness, { props: { client, component: VmActions, props: { vm } } });
	return { client, fetcher, view };
}

const button = (name: string) => screen.getByRole('button', { name });
const typeName = (value: string) =>
	fireEvent.input(screen.getByLabelText(`Type ${running.name} to force it off`), {
		target: { value }
	});

afterEach(() => vi.unstubAllGlobals());

describe('the power buttons', () => {
	it('offers Shut down and Force off for a running VM, and Start for a shut-off one', () => {
		show(running);
		expect(button('Shut down alpha')).toBeInTheDocument();
		expect(button('Force off alpha')).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Start alpha' })).toBeNull();
		show(shutoff);
		expect(button('Start beta')).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Shut down beta' })).toBeNull();
		expect(screen.queryByRole('button', { name: 'Force off beta' })).toBeNull();
	});

	it('starts a VM with the CSRF token and no body', async () => {
		const { fetcher } = show(shutoff);
		await fireEvent.click(button('Start beta'));
		await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
		const [path, init] = fetcher.mock.calls[0];
		expect(path).toBe(`/api/vms/${shutoff.uuid}/actions/start`);
		expect(init?.method).toBe('POST');
		expect((init?.headers as Record<string, string>)['x-csrf-token']).toBe('csrf1');
		expect(init?.body).toBeUndefined();
	});

	it('says that a shutdown waits for the guest, until the state changes', async () => {
		const { view } = show(running);
		await fireEvent.click(button('Shut down alpha'));
		expect(await screen.findByRole('status')).toHaveTextContent(
			'Shutdown requested. alpha stops when its guest finishes.'
		);
		// rerender unwraps a top-level `props` key, so the harness's own
		// `props` prop needs a second level.
		await view.rerender({ props: { props: { vm: { ...running, state: 'shutoff' } } } });
		expect(await screen.findByRole('button', { name: 'Start alpha' })).toBeInTheDocument();
		await waitFor(() => expect(screen.queryByRole('status')).toBeNull());
	});

	it('drops the shutdown note when the VM stopped before the answer came', async () => {
		let answer: (res: Response) => void = () => {};
		const { view } = show(running);
		vi.stubGlobal(
			'fetch',
			vi.fn(() => new Promise<Response>((resolve) => (answer = resolve)))
		);
		await fireEvent.click(button('Shut down alpha'));
		// The event refreshes the list first, then the 204 arrives.
		await view.rerender({ props: { props: { vm: { ...running, state: 'shutoff' } } } });
		answer(new Response(null, { status: 204 }));
		expect(await screen.findByRole('button', { name: 'Start alpha' })).toBeEnabled();
		expect(screen.queryByRole('status')).toBeNull();
	});

	it('closes the Force off field when the VM stops for another reason', async () => {
		const { view } = show(running);
		await fireEvent.click(button('Force off alpha'));
		await typeName('alpha');
		await view.rerender({ props: { props: { vm: { ...running, state: 'shutoff' } } } });
		expect(await screen.findByRole('button', { name: 'Start alpha' })).toBeInTheDocument();
		expect(screen.queryByLabelText('Type alpha to force it off')).toBeNull();
	});

	it('forces off only after the exact name is typed, and sends it', async () => {
		const { fetcher } = show(running);
		await fireEvent.click(button('Force off alpha'));
		const force = button('Force off');
		expect(force).toBeDisabled();
		await typeName('ALPHA');
		expect(force).toBeDisabled();
		// jsdom still runs a click on a disabled button: nothing may go out.
		await fireEvent.click(force);
		expect(fetcher).not.toHaveBeenCalled();
		await typeName('alpha');
		expect(force).toBeEnabled();
		await fireEvent.click(force);
		await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
		const [path, init] = fetcher.mock.calls[0];
		expect(path).toBe(`/api/vms/${running.uuid}/actions/force-off`);
		expect(JSON.parse(init?.body as string)).toEqual({ confirm: 'alpha' });
		// Done: the row shows the normal buttons again.
		expect(await screen.findByRole('button', { name: 'Force off alpha' })).toBeInTheDocument();
	});

	it('cancels a force off without sending anything', async () => {
		const { fetcher } = show(running);
		await fireEvent.click(button('Force off alpha'));
		await typeName('alpha');
		await fireEvent.click(button('Cancel'));
		expect(button('Force off alpha')).toBeInTheDocument();
		expect(fetcher).not.toHaveBeenCalled();
	});

	it('shows the server error as a sentence', async () => {
		show(
			shutoff,
			() => new Response(JSON.stringify({ error: 'the VM is running already' }), { status: 409 })
		);
		await fireEvent.click(button('Start beta'));
		expect(await screen.findByRole('alert')).toHaveTextContent('The VM is running already.');
		expect(button('Start beta')).toBeEnabled();
	});

	it('drops the session on a 401', async () => {
		const { client } = show(
			shutoff,
			() => new Response(JSON.stringify({ error: 'log in first' }), { status: 401 })
		);
		await fireEvent.click(button('Start beta'));
		await waitFor(() => expect(client.getQueryData(keys.session)).toBeNull());
	});
});

describe('reboot, pause, and resume', () => {
	it('offers Reboot and Pause for a running VM, and Resume for a paused one', async () => {
		show(running);
		expect(button('Reboot alpha')).toBeInTheDocument();
		expect(button('Pause alpha')).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Resume alpha' })).toBeNull();
		show({ ...shutoff, state: 'paused' });
		expect(button('Resume beta')).toBeInTheDocument();
		expect(button('Force off beta')).toBeInTheDocument();
		for (const name of ['Reboot beta', 'Pause beta', 'Shut down beta', 'Start beta']) {
			expect(screen.queryByRole('button', { name })).toBeNull();
		}
	});

	it.each([
		['Reboot alpha', running, 'reboot'],
		['Pause alpha', running, 'pause'],
		['Resume alpha', { ...running, state: 'paused' as const }, 'resume']
	])('%s sends its action with no body', async (label, vm, action) => {
		const { fetcher } = show(vm);
		await fireEvent.click(button(label));
		await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
		const [path, init] = fetcher.mock.calls[0];
		expect(path).toBe(`/api/vms/${running.uuid}/actions/${action}`);
		expect(init?.method).toBe('POST');
		expect(init?.body).toBeUndefined();
	});
});

describe('a guest that ignores the ACPI request', () => {
	afterEach(() => vi.useRealTimers());

	it('gets a Force off offer after 120 seconds', async () => {
		vi.useFakeTimers({ shouldAdvanceTime: true });
		show(running);
		await fireEvent.click(button('Shut down alpha'));
		expect(await screen.findByRole('status')).toHaveTextContent('Shutdown requested.');
		await vi.advanceTimersByTimeAsync(119_999);
		expect(screen.getByRole('status')).toHaveTextContent('Shutdown requested.');
		await vi.advanceTimersByTimeAsync(1);
		expect(screen.getByRole('status')).toHaveTextContent(
			'alpha did not shut down within 120 seconds. Its guest may ignore the ACPI request.'
		);
		await fireEvent.click(button('Force off instead'));
		expect(screen.getByLabelText('Type alpha to force it off')).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Force off instead' })).toBeNull();
	});

	it('gets no offer when the guest stops in time', async () => {
		vi.useFakeTimers({ shouldAdvanceTime: true });
		const { view } = show(running);
		await fireEvent.click(button('Shut down alpha'));
		await screen.findByRole('status');
		await vi.advanceTimersByTimeAsync(60_000);
		await view.rerender({ props: { props: { vm: { ...running, state: 'shutoff' } } } });
		await vi.advanceTimersByTimeAsync(120_000);
		expect(screen.queryByRole('status')).toBeNull();
		expect(screen.queryByRole('button', { name: 'Force off instead' })).toBeNull();
		// The old timer is gone: a new request waits its own 120 seconds.
		await view.rerender({ props: { props: { vm: running } } });
		await fireEvent.click(button('Shut down alpha'));
		expect(await screen.findByRole('status')).toHaveTextContent('Shutdown requested.');
	});
});

describe('a known libvirt error', () => {
	it('shows the AppArmor rule and its 2 commands for the getfd error', async () => {
		show(
			running,
			() =>
				new Response(
					JSON.stringify({
						error:
							"libvirt: internal error: unable to execute QEMU command 'getfd': No file descriptor supplied via SCM_RIGHTS",
						cause: 'AppArmor on this host blocks /dev/vhost-net.',
						fix: 'Add the rule `/dev/vhost-net rw,` to /etc/apparmor.d/local/abstractions/libvirt-qemu.',
						commands: [
							'sudo mkdir -p /etc/apparmor.d/local/abstractions',
							'printf x | sudo tee -a y'
						]
					}),
					{ status: 502 }
				)
		);
		await fireEvent.click(button('Reboot alpha'));
		const alert = await screen.findByRole('alert');
		expect(alert).toHaveTextContent('Add the rule `/dev/vhost-net rw,`');
		expect(screen.getByLabelText('Commands to run on the host').textContent).toBe(
			'sudo mkdir -p /etc/apparmor.d/local/abstractions\nprintf x | sudo tee -a y'
		);
	});

	it('ignores an explanation with a missing part', async () => {
		show(
			running,
			() =>
				new Response(JSON.stringify({ error: 'the VM is paused', cause: 'x', commands: [] }), {
					status: 409
				})
		);
		await fireEvent.click(button('Reboot alpha'));
		expect((await screen.findByRole('alert')).textContent?.trim()).toBe('The VM is paused.');
	});
});
