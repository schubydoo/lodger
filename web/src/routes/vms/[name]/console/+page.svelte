<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { createQuery } from '@tanstack/svelte-query';
	import RFB from '@novnc/novnc';
	import { fetchVms, keys } from '$lib/api';
	import { statusText, vncUrl, type ConsoleStatus } from '$lib/console';
	import { nextTicket, withTicket } from '$lib/session';

	const name = $derived(page.params.name ?? '');
	const vms = createQuery(() => ({ queryKey: keys.vms, queryFn: () => fetchVms() }));
	const vm = $derived(vms.data?.find((v) => v.name === name));

	let screen = $state<HTMLDivElement>();
	let rfb = $state<RFB>();
	let status = $state<ConsoleStatus>('connecting');

	// The UUID to connect to, or `undefined` while the VM does not run. A
	// string compares by value, so a change to another field of the VM, such
	// as its memory, does not reconnect the console.
	const target = $derived(vm?.state === 'running' ? vm.uuid : undefined);

	// Connect once the VM is known to run, with a fresh ticket. The socket
	// closes when the page goes away, and the server then closes the display
	// socket too. The server also closes it when the session ends.
	$effect(() => {
		if (!screen || !target) return;
		const element = screen;
		const url = vncUrl(window.location, target);
		let client: RFB | undefined;
		let gone = false;
		status = 'connecting';
		nextTicket().then(
			(ticket) => {
				if (gone) return;
				client = new RFB(element, withTicket(url, ticket), { wsProtocols: ['binary'] });
				client.scaleViewport = true;
				client.background = 'transparent';
				client.addEventListener('connect', () => (status = 'connected'));
				client.addEventListener('disconnect', (e) => {
					status = (e as CustomEvent<{ clean: boolean }>).detail.clean ? 'closed' : 'failed';
				});
				rfb = client;
			},
			() => {
				if (!gone) status = 'failed';
			}
		);
		return () => {
			gone = true;
			rfb = undefined;
			client?.disconnect();
		};
	});
</script>

<svelte:head><title>{name} console · Lodger</title></svelte:head>

<div class="mb-4 flex flex-wrap items-center gap-4">
	<h1 class="text-2xl font-semibold">{name}</h1>
	<a href={resolve('/vms')} class="text-sm underline underline-offset-4">Back to the list</a>
</div>

{#if vms.isPending}
	<p>Loading…</p>
{:else if vms.isError}
	<p role="alert">Could not load the virtual machines: {vms.error.message}</p>
{:else if !vm}
	<p role="alert">No virtual machine is called {name}.</p>
{:else if vm.state !== 'running'}
	<p>{name} is not running, so it has no screen to show.</p>
{:else}
	<div class="mb-3 flex flex-wrap items-center gap-4">
		<p role="status" class="text-sm text-muted-foreground">{statusText(status)}</p>
		<button
			type="button"
			class="rounded-md border px-3 py-1.5 text-sm hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none disabled:opacity-50"
			disabled={status !== 'connected'}
			onclick={() => rfb?.sendCtrlAltDel()}
		>
			Send Ctrl+Alt+Del
		</button>
	</div>
	<!-- noVNC draws a canvas here. The canvas shows only pixels, so it cannot
	     meet WCAG (PRD 5.4); the serial console is the text path. -->
	<div
		bind:this={screen}
		aria-label="Screen of {name}"
		role="application"
		class="h-[70vh] w-full overflow-hidden rounded-md border bg-black"
	></div>
{/if}
