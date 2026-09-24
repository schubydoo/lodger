<script lang="ts">
	import { resolve } from '$app/paths';
	import { createQuery } from '@tanstack/svelte-query';
	import * as Table from '$lib/components/ui/table';
	import PoolCreate from '$lib/components/PoolCreate.svelte';
	import { fetchPools, formatBytes, keys } from '$lib/api';

	const pools = createQuery(() => ({ queryKey: keys.pools, queryFn: () => fetchPools() }));
</script>

<svelte:head><title>Storage · Lodger</title></svelte:head>

<h1 class="mb-6 text-2xl font-semibold">Storage pools</h1>

{#if pools.isPending}
	<p>Loading the storage pools…</p>
{:else if pools.isError}
	<p role="alert">Could not load the storage pools: {pools.error.message}</p>
{:else if pools.data.length === 0}
	<p>libvirt has no storage pools on this host.</p>
{:else}
	<Table.Root label="Storage pools">
		<Table.Caption class="sr-only">{pools.data.length} storage pools on this host</Table.Caption>
		<Table.Header>
			<Table.Row>
				<Table.Head scope="col">Name</Table.Head>
				<Table.Head scope="col">State</Table.Head>
				<Table.Head scope="col" class="text-right">Size</Table.Head>
				<Table.Head scope="col" class="text-right">Free</Table.Head>
				<Table.Head scope="col">Autostart</Table.Head>
			</Table.Row>
		</Table.Header>
		<Table.Body>
			{#each pools.data as pool (pool.uuid)}
				<Table.Row>
					<Table.Cell class="font-medium">
						<a
							href={resolve('/storage/[name]', { name: pool.name })}
							class="rounded-sm underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
						>
							{pool.name}
						</a>
					</Table.Cell>
					<Table.Cell>{pool.state}</Table.Cell>
					<Table.Cell class="text-right tabular-nums">
						{pool.state === 'running' ? formatBytes(pool.capacity_bytes) : '–'}
					</Table.Cell>
					<Table.Cell class="text-right tabular-nums">
						{pool.state === 'running' ? formatBytes(pool.available_bytes) : '–'}
					</Table.Cell>
					<Table.Cell>{pool.autostart ? 'Yes' : 'No'}</Table.Cell>
				</Table.Row>
			{/each}
		</Table.Body>
	</Table.Root>
{/if}

<h2 class="mt-10 mb-4 text-lg font-semibold">New pool</h2>
<PoolCreate />
