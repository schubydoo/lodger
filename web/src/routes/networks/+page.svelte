<script lang="ts">
	import { resolve } from '$app/paths';
	import { createQuery } from '@tanstack/svelte-query';
	import * as Table from '$lib/components/ui/table';
	import NetworkCreate from '$lib/components/NetworkCreate.svelte';
	import { fetchNetworks, keys } from '$lib/api';

	const networks = createQuery(() => ({ queryKey: keys.networks, queryFn: () => fetchNetworks() }));
</script>

<svelte:head><title>Networks · Lodger</title></svelte:head>

<h1 class="mb-6 text-2xl font-semibold">Virtual networks</h1>

{#if networks.isPending}
	<p>Loading the networks…</p>
{:else if networks.isError}
	<p role="alert">Could not load the networks: {networks.error.message}</p>
{:else if networks.data.length === 0}
	<p>libvirt has no virtual networks on this host.</p>
{:else}
	<Table.Root label="Virtual networks">
		<Table.Caption class="sr-only"
			>{networks.data.length} virtual networks on this host</Table.Caption
		>
		<Table.Header>
			<Table.Row>
				<Table.Head scope="col">Name</Table.Head>
				<Table.Head scope="col">State</Table.Head>
				<Table.Head scope="col">Bridge</Table.Head>
				<Table.Head scope="col">Autostart</Table.Head>
			</Table.Row>
		</Table.Header>
		<Table.Body>
			{#each networks.data as network (network.uuid)}
				<Table.Row>
					<Table.Cell class="font-medium">
						<a
							href={resolve('/networks/[name]', { name: encodeURIComponent(network.name) })}
							class="rounded-sm underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
						>
							{network.name}
						</a>
					</Table.Cell>
					<Table.Cell>{network.active ? 'running' : 'inactive'}</Table.Cell>
					<Table.Cell>{network.bridge ?? '–'}</Table.Cell>
					<Table.Cell>{network.autostart ? 'Yes' : 'No'}</Table.Cell>
				</Table.Row>
			{/each}
		</Table.Body>
	</Table.Root>
{/if}

<h2 class="mt-10 mb-4 text-lg font-semibold">New network</h2>
<NetworkCreate />
