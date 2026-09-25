<script lang="ts">
	import { page } from '$app/state';
	import { vanishing } from '$lib/vanishing.svelte';
	import { createQuery } from '@tanstack/svelte-query';
	import NetworkManage from '$lib/components/NetworkManage.svelte';
	import { fetchNetwork, fetchNetworks, keys } from '$lib/api';

	const name = $derived(page.params.name ?? '');
	const networks = createQuery(() => ({ queryKey: keys.networks, queryFn: () => fetchNetworks() }));
	const listed = $derived(networks.data?.find((n) => n.name === name));
	// The detail needs the UUID, which the list gives.
	const detail = createQuery(() => ({
		queryKey: keys.network(listed?.uuid ?? ''),
		queryFn: () => fetchNetwork(listed!.uuid),
		enabled: listed !== undefined && !vanishing.has(listed.uuid)
	}));
	const modeText = (mode: string | null) =>
		mode === null ? 'Isolated' : mode === 'nat' ? 'NAT' : mode === 'bridge' ? 'Host bridge' : mode;
</script>

<svelte:head><title>{name} · Lodger</title></svelte:head>

{#if networks.isPending || (listed && detail.isPending)}
	<p>Loading the network…</p>
{:else if networks.isError}
	<p role="alert">Could not load the networks: {networks.error.message}</p>
{:else if !listed}
	<p role="alert">libvirt has no virtual network called {name}.</p>
{:else if detail.isError}
	<p role="alert">Could not load the network: {detail.error.message}</p>
{:else if detail.data}
	{@const network = detail.data}
	<h1 class="mb-6 text-2xl font-semibold">{network.name}</h1>
	<dl class="mb-6 grid max-w-xl grid-cols-[auto_1fr] gap-x-6 gap-y-2 text-sm">
		<dt class="text-muted-foreground">State</dt>
		<dd>{network.active ? 'running' : 'inactive'}</dd>
		<dt class="text-muted-foreground">Kind</dt>
		<dd>{modeText(network.mode)}</dd>
		<dt class="text-muted-foreground">Bridge</dt>
		<dd>{network.bridge ?? '–'}</dd>
		<dt class="text-muted-foreground">Subnets</dt>
		<dd>{network.subnets.length > 0 ? network.subnets.join(', ') : '–'}</dd>
		<dt class="text-muted-foreground">Autostart</dt>
		<dd>{network.autostart ? 'On' : 'Off'}</dd>
	</dl>
	<NetworkManage {network} />
{/if}
