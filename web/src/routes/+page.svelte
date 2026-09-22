<script lang="ts">
	import { resolve } from '$app/paths';
	import { createQuery } from '@tanstack/svelte-query';
	import * as Card from '$lib/components/ui/card';
	import { fetchHost, formatKib, keys } from '$lib/api';

	const host = createQuery(() => ({ queryKey: keys.host, queryFn: () => fetchHost() }));
</script>

<svelte:head><title>Overview · Lodger</title></svelte:head>

<h1 class="mb-6 text-2xl font-semibold">
	{host.data?.info?.hostname ?? 'Host overview'}
</h1>

{#if host.isPending}
	<p>Loading the host…</p>
{:else if host.isError}
	<p role="alert">Could not load the host: {host.error.message}</p>
{:else}
	{@const data = host.data}
	<div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
		<Card.Root>
			<Card.Header>
				<Card.Description>Virtual machines</Card.Description>
				<Card.Title class="text-3xl">{data.vms.total}</Card.Title>
			</Card.Header>
			<Card.Content>
				<a href={resolve('/vms')} class="text-sm underline underline-offset-4">
					{data.vms.running} running
				</a>
			</Card.Content>
		</Card.Root>
		<Card.Root>
			<Card.Header>
				<Card.Description>Storage pools</Card.Description>
				<Card.Title class="text-3xl">{data.pools}</Card.Title>
			</Card.Header>
		</Card.Root>
		<Card.Root>
			<Card.Header>
				<Card.Description>Networks</Card.Description>
				<Card.Title class="text-3xl">{data.networks}</Card.Title>
			</Card.Header>
		</Card.Root>
		<Card.Root>
			<Card.Header>
				<Card.Description>Host</Card.Description>
				{#if data.info}
					<Card.Title>{data.info.cpus} CPUs · {formatKib(data.info.memory_kib)}</Card.Title>
				{:else}
					<Card.Title>Unknown</Card.Title>
				{/if}
			</Card.Header>
			<Card.Content class="text-sm text-muted-foreground">
				{#if data.info}
					libvirt {data.info.libvirt_version}
				{:else if data.info_error}
					{data.info_error}
				{/if}
			</Card.Content>
		</Card.Root>
	</div>
{/if}
