<script lang="ts">
	import { createQuery } from '@tanstack/svelte-query';
	import * as Table from '$lib/components/ui/table';
	import StateBadge from '$lib/components/StateBadge.svelte';
	import { fetchVms, formatKib, keys } from '$lib/api';

	const vms = createQuery(() => ({ queryKey: keys.vms, queryFn: () => fetchVms() }));
</script>

<svelte:head><title>Virtual machines · Lodger</title></svelte:head>

<h1 class="mb-6 text-2xl font-semibold">Virtual machines</h1>

{#if vms.isPending}
	<p>Loading the virtual machines…</p>
{:else if vms.isError}
	<p role="alert">Could not load the virtual machines: {vms.error.message}</p>
{:else if vms.data.length === 0}
	<p>libvirt has no virtual machines on this host.</p>
{:else}
	<Table.Root label="Virtual machines">
		<Table.Caption class="sr-only">
			{vms.data.length} virtual machines on this host
		</Table.Caption>
		<Table.Header>
			<Table.Row>
				<Table.Head scope="col">Name</Table.Head>
				<Table.Head scope="col">State</Table.Head>
				<Table.Head scope="col" class="text-right">vCPUs</Table.Head>
				<Table.Head scope="col" class="text-right">Memory</Table.Head>
				<Table.Head scope="col">Autostart</Table.Head>
			</Table.Row>
		</Table.Header>
		<Table.Body>
			{#each vms.data as vm (vm.uuid)}
				<Table.Row>
					<Table.Cell class="font-medium">
						{vm.name}
						{#if !vm.persistent}
							<span class="ml-2 text-xs text-muted-foreground">(transient)</span>
						{/if}
					</Table.Cell>
					<Table.Cell><StateBadge state={vm.state} /></Table.Cell>
					<Table.Cell class="text-right tabular-nums">{vm.vcpus}</Table.Cell>
					<Table.Cell class="text-right tabular-nums">{formatKib(vm.memory_kib)}</Table.Cell>
					<Table.Cell>{vm.autostart ? 'Yes' : 'No'}</Table.Cell>
				</Table.Row>
			{/each}
		</Table.Body>
	</Table.Root>
{/if}
