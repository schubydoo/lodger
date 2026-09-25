<script lang="ts">
	import { page } from '$app/state';
	import { vanishing } from '$lib/vanishing.svelte';
	import { createQuery } from '@tanstack/svelte-query';
	import PoolManage from '$lib/components/PoolManage.svelte';
	import PoolVolumes from '$lib/components/PoolVolumes.svelte';
	import { fetchPool, fetchPools, formatBytes, keys } from '$lib/api';

	const name = $derived(page.params.name ?? '');
	const pools = createQuery(() => ({ queryKey: keys.pools, queryFn: () => fetchPools() }));
	const listed = $derived(pools.data?.find((p) => p.name === name));
	// The detail needs the UUID, which the list gives.
	const detail = createQuery(() => ({
		queryKey: keys.pool(listed?.uuid ?? ''),
		queryFn: () => fetchPool(listed!.uuid),
		enabled: listed !== undefined && !vanishing.has(listed.uuid)
	}));
</script>

<svelte:head><title>{name} · Lodger</title></svelte:head>

{#if pools.isPending || (listed && detail.isPending)}
	<p>Loading the storage pool…</p>
{:else if pools.isError}
	<p role="alert">Could not load the storage pools: {pools.error.message}</p>
{:else if !listed}
	<p role="alert">libvirt has no storage pool called {name}.</p>
{:else if detail.isError}
	<p role="alert">Could not load the storage pool: {detail.error.message}</p>
{:else if detail.data}
	{@const pool = detail.data}
	<h1 class="mb-6 text-2xl font-semibold">{pool.name}</h1>
	<dl class="mb-6 grid max-w-xl grid-cols-[auto_1fr] gap-x-6 gap-y-2 text-sm">
		<dt class="text-muted-foreground">State</dt>
		<dd>{pool.state}</dd>
		<dt class="text-muted-foreground">Kind</dt>
		<dd>{pool.kind === 'netfs' ? 'NFS share' : pool.kind === 'dir' ? 'Folder' : pool.kind}</dd>
		{#if pool.nfs}
			<dt class="text-muted-foreground">NFS source</dt>
			<dd class="break-all">{pool.nfs.host}:{pool.nfs.export}</dd>
		{/if}
		<dt class="text-muted-foreground">Folder</dt>
		<dd class="break-all">{pool.path ?? '–'}</dd>
		{#if pool.state === 'running'}
			<dt class="text-muted-foreground">Size</dt>
			<dd class="tabular-nums">
				{formatBytes(pool.allocation_bytes)} used of {formatBytes(pool.capacity_bytes)}
			</dd>
		{/if}
		<dt class="text-muted-foreground">Autostart</dt>
		<dd>{pool.autostart ? 'On' : 'Off'}</dd>
	</dl>
	<PoolManage {pool}>
		{#if pool.state === 'running'}
			<PoolVolumes {pool} />
		{:else}
			<p class="mt-8 text-sm">Start the pool to see and change its volumes.</p>
		{/if}
	</PoolManage>
{/if}
