<!-- The New network form (PRD F8). A NAT or isolated network needs a
     private subnet; the host takes its first address, and DHCP hands out the
     rest. A bridge network needs one of the host's bridges, which Lodger only
     lists: it never creates or changes a host interface. Autostart is on
     unless the user turns it off. -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { noAutofill } from '$lib/no-autofill';
	import { resolve } from '$app/paths';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import Problem from '$lib/components/Problem.svelte';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import {
		ApiError,
		createNetwork,
		fetchHostBridges,
		keys,
		type NewNetwork,
		type Session,
		fetchNetwork
	} from '$lib/api';

	const client = useQueryClient();
	const bridges = createQuery(() => ({
		queryKey: keys.hostBridges,
		queryFn: () => fetchHostBridges()
	}));

	let mode = $state<'nat' | 'isolated' | 'bridge'>('nat');
	let name = $state('');
	let subnet = $state('');
	let bridge = $state('');
	// The form offers the bridges that nothing owns. A libvirt network's bridge
	// or a Docker bridge is almost never the LAN, but a host can name its LAN
	// bridge freely, so the others stay one click away.
	let everyBridge = $state(false);
	const offered = $derived((bridges.data ?? []).filter((b) => everyBridge || b.owner === null));
	let autostart = $state(true);
	let busy = $state(false);
	let problem = $state<unknown>(null);

	// A bridge counts only while the list shows it: a bridge picked under "Show
	// every bridge" is not sent after the switch goes off again.
	const ready = $derived(
		name.trim() !== '' &&
			(mode === 'bridge' ? offered.some((b) => b.name === bridge) : subnet.trim() !== '')
	);

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (busy || !ready) return;
		problem = null;
		busy = true;
		const network: NewNetwork =
			mode === 'bridge'
				? { mode, name: name.trim(), bridge, autostart }
				: { mode, name: name.trim(), subnet: subnet.trim(), autostart };
		try {
			const { uuid } = await createNetwork(network, {
				csrf: client.getQueryData<Session | null>(keys.session)?.csrf_token
			});
			// The new page finds the network in the list: refresh it first. Fetch the
			// detail too, so the page opens with its data and shows no loading step.
			await client.invalidateQueries({ queryKey: keys.networks });
			await client.prefetchQuery({
				queryKey: keys.network(uuid),
				queryFn: () => fetchNetwork(uuid)
			});
			await goto(resolve('/networks/[name]', { name: encodeURIComponent(network.name) }));
		} catch (e) {
			if (e instanceof ApiError && e.status === 401) client.setQueryData(keys.session, null);
			problem = e;
		} finally {
			busy = false;
		}
	}
</script>

<form class="flex max-w-xl flex-col gap-4" onsubmit={submit}>
	<fieldset class="flex flex-wrap gap-4">
		<legend class="mb-1 text-sm font-medium">Kind</legend>
		<label class="flex items-center gap-2 text-sm">
			<input type="radio" name="mode" value="nat" bind:group={mode} /> NAT
		</label>
		<label class="flex items-center gap-2 text-sm">
			<input type="radio" name="mode" value="isolated" bind:group={mode} /> Isolated
		</label>
		<label class="flex items-center gap-2 text-sm">
			<input type="radio" name="mode" value="bridge" bind:group={mode} /> Host bridge
		</label>
	</fieldset>
	<div class="flex flex-col gap-1">
		<Label for="network-name">Name</Label>
		<Input id="network-name" {...noAutofill} spellcheck={false} bind:value={name} />
	</div>
	{#if mode === 'bridge'}
		{#if bridges.isPending}
			<p class="text-sm">Loading the host bridges…</p>
		{:else if bridges.isError}
			<p role="alert" class="text-sm">Could not load the host bridges: {bridges.error.message}</p>
		{:else if bridges.data.length === 0}
			<p role="note" class="text-sm">
				This host has no bridge. A host bridge must exist first: create it on the host, for example
				with NetworkManager or systemd-networkd. Lodger never changes the host's network.
			</p>
		{:else}
			{#if offered.length === 0}
				<p role="note" class="text-sm">
					Every bridge on this host belongs to a libvirt network or to Docker. A host bridge for the
					LAN must exist first: create it on the host, for example with NetworkManager or
					systemd-networkd.
				</p>
			{:else}
				<div class="flex flex-col gap-1">
					<Label for="network-bridge">Host bridge</Label>
					<select
						id="network-bridge"
						class="h-9 rounded-md border bg-background px-2 text-sm"
						bind:value={bridge}
					>
						<option value="" disabled>Pick a bridge</option>
						{#each offered as b (b.name)}
							<option value={b.name}>{b.owner ? `${b.name} (${b.owner})` : b.name}</option>
						{/each}
					</select>
				</div>
			{/if}
			{#if bridges.data.some((b) => b.owner !== null)}
				<label class="flex items-center gap-2 text-sm">
					<input type="checkbox" bind:checked={everyBridge} />
					Show every bridge, also those of libvirt networks and Docker
				</label>
			{/if}
		{/if}
	{:else}
		<div class="flex flex-col gap-1">
			<Label for="network-subnet">Subnet</Label>
			<Input
				id="network-subnet"
				placeholder="192.168.150.0/24"
				{...noAutofill}
				spellcheck={false}
				bind:value={subnet}
			/>
		</div>
	{/if}
	<label class="flex items-center gap-2 text-sm">
		<input type="checkbox" bind:checked={autostart} />
		Start the network when the host boots
	</label>
	<div>
		<Button type="submit" disabled={!ready || busy}>
			{busy ? 'Creating…' : 'Create network'}
		</Button>
	</div>
	{#if problem !== null}
		<Problem error={problem} />
	{/if}
</form>
