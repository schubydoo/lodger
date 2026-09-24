import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import VmManage from './VmManage.svelte';
import QueryHarness from '$lib/test/QueryHarness.svelte';
import { testClient, vms } from '$lib/test/fixtures';
import { keys, type Vm } from '$lib/api';
import { lastDeletion } from '$lib/deletion.svelte';

const nav = vi.hoisted(() => ({ goto: vi.fn(async () => {}) }));
vi.mock('$app/navigation', () => ({ goto: nav.goto }));

const [running, shutoff] = vms;

function show(vm: Vm, respond: () => Response = () => new Response(null, { status: 204 })) {
	const fetcher = vi.fn<(path: string, init?: RequestInit) => Promise<Response>>(async () =>
		respond()
	);
	vi.stubGlobal('fetch', fetcher);
	const client = testClient();
	client.setQueryData(keys.session, { username: 'admin', csrf_token: 'csrf1' });
	const view = render(QueryHarness, { props: { client, component: VmManage, props: { vm } } });
	return { client, fetcher, view };
}

const button = (name: string) => screen.getByRole('button', { name });
const typeName = (value: string) =>
	fireEvent.input(screen.getByLabelText(`Type ${shutoff.name} to delete it`), {
		target: { value }
	});

beforeEach(() => {
	nav.goto.mockClear();
	lastDeletion.current = null;
});
afterEach(() => vi.unstubAllGlobals());

describe('autostart', () => {
	it('turns autostart off for a VM that has it, with the CSRF token', async () => {
		const { fetcher } = show(running);
		expect(screen.getByText(/Autostart is on/)).toBeInTheDocument();
		await fireEvent.click(button('Turn autostart off'));
		await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
		const [path, init] = fetcher.mock.calls[0];
		expect(path).toBe(`/api/vms/${running.uuid}`);
		expect(init?.method).toBe('PATCH');
		expect((init?.headers as Record<string, string>)['x-csrf-token']).toBe('csrf1');
		expect(JSON.parse(init?.body as string)).toEqual({ autostart: false });
	});

	it('turns it on for a VM without it', async () => {
		const { fetcher } = show(shutoff);
		await fireEvent.click(button('Turn autostart on'));
		await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
		expect(JSON.parse(fetcher.mock.calls[0][1]?.body as string)).toEqual({ autostart: true });
	});

	it('shows the server error as a sentence', async () => {
		show(running, () => new Response(JSON.stringify({ error: 'no such VM' }), { status: 404 }));
		await fireEvent.click(button('Turn autostart off'));
		expect(await screen.findByRole('alert')).toHaveTextContent('No such VM.');
	});
});

describe('delete', () => {
	it('needs a shut-off VM', () => {
		show(running);
		expect(screen.getByText('Shut alpha down to delete it.')).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Delete alpha' })).toBeNull();
	});

	it('deletes only after the exact name is typed, then shows the list', async () => {
		const removal = {
			removed: ['/p/beta.qcow2'],
			skipped: [{ path: '/p/base.qcow2', reason: 'used_by', vm: 'gamma' }]
		};
		const { fetcher } = show(shutoff, () => new Response(JSON.stringify(removal)));
		await fireEvent.click(button('Delete beta'));
		const remove = button('Delete');
		expect(remove).toBeDisabled();
		await typeName('BETA');
		// jsdom still runs a click on a disabled button: nothing may go out.
		await fireEvent.click(remove);
		expect(fetcher).not.toHaveBeenCalled();
		await fireEvent.click(screen.getByLabelText('Also delete its volumes that no other VM uses'));
		await typeName('beta');
		expect(remove).toBeEnabled();
		await fireEvent.click(remove);
		await waitFor(() => expect(nav.goto).toHaveBeenCalledWith('/vms'));
		const [path, init] = fetcher.mock.calls[0];
		expect(path).toBe(`/api/vms/${shutoff.uuid}`);
		expect(init?.method).toBe('DELETE');
		expect(JSON.parse(init?.body as string)).toEqual({ confirm: 'beta', remove_volumes: true });
		expect(lastDeletion.current).toEqual({ name: 'beta', removal });
	});

	it('keeps the volumes unless the box is ticked', async () => {
		const { fetcher } = show(
			shutoff,
			() => new Response(JSON.stringify({ removed: [], skipped: [] }))
		);
		await fireEvent.click(button('Delete beta'));
		await typeName('beta');
		await fireEvent.click(button('Delete'));
		await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
		expect(JSON.parse(fetcher.mock.calls[0][1]?.body as string)).toEqual({
			confirm: 'beta',
			remove_volumes: false
		});
	});

	it('stays on the page with the error when the delete fails', async () => {
		show(
			shutoff,
			() => new Response(JSON.stringify({ error: 'shut the VM down first' }), { status: 409 })
		);
		await fireEvent.click(button('Delete beta'));
		await typeName('beta');
		await fireEvent.click(button('Delete'));
		expect(await screen.findByRole('alert')).toHaveTextContent('Shut the VM down first.');
		expect(nav.goto).not.toHaveBeenCalled();
		expect(lastDeletion.current).toBeNull();
	});

	it('closes the field when the VM starts meanwhile', async () => {
		const { view } = show(shutoff);
		await fireEvent.click(button('Delete beta'));
		await typeName('beta');
		await view.rerender({ props: { props: { vm: { ...shutoff, state: 'running' } } } });
		expect(await screen.findByText('Shut beta down to delete it.')).toBeInTheDocument();
		// Stopped again: the field does not come back with the old name in it.
		await view.rerender({ props: { props: { vm: shutoff } } });
		expect(await screen.findByRole('button', { name: 'Delete beta' })).toBeInTheDocument();
		expect(screen.queryByLabelText('Type beta to delete it')).toBeNull();
	});

	it('cancels without sending anything', async () => {
		const { fetcher } = show(shutoff);
		await fireEvent.click(button('Delete beta'));
		await typeName('beta');
		await fireEvent.click(button('Cancel'));
		expect(button('Delete beta')).toBeInTheDocument();
		expect(fetcher).not.toHaveBeenCalled();
	});
});
