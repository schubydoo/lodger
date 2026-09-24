<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import StateBadge from '$lib/components/StateBadge.svelte';
	import VmActions from '$lib/components/VmActions.svelte';
	import VmManage from '$lib/components/VmManage.svelte';
	import { fetchVms, formatKib, formatRate, keys, type VmStats } from '$lib/api';
	import { wantStats } from '$lib/events';

	const name = $derived(page.params.name ?? '');
	const vms = createQuery(() => ({ queryKey: keys.vms, queryFn: () => fetchVms() }));
	const vm = $derived(vms.data?.find((v) => v.name === name));

	// The events socket fills this query while the page subscribes. Nothing
	// fetches it, so the query never runs its function.
	const client = useQueryClient();
	const stats = createQuery(() => ({
		queryKey: keys.stats,
		queryFn: () => client.getQueryData<VmStats[]>(keys.stats) ?? [],
		enabled: false
	}));
	const mine = $derived(vm ? stats.data?.find((s) => s.uuid === vm.uuid) : undefined);

	// Live stats only while this page is open (TAD 6.2).
	$effect(() => wantStats());

	const unknown = '–';
	const rate = (v: number | null) => (v === null ? unknown : formatRate(v));
	const memory = (s: VmStats) =>
		s.memory_used_kib !== null && s.memory_kib !== null
			? `${formatKib(s.memory_used_kib)} of ${formatKib(s.memory_kib)}`
			: s.memory_kib !== null
				? `${formatKib(s.memory_kib)} (the guest reports no use)`
				: unknown;
</script>

<svelte:head><title>{name} · Lodger</title></svelte:head>

{#if vms.isPending}
	<p>Loading the virtual machine…</p>
{:else if vms.isError}
	<p role="alert">Could not load the virtual machines: {vms.error.message}</p>
{:else if !vm}
	<p role="alert">libvirt has no virtual machine called {name}.</p>
{:else}
	<div class="mb-6 flex flex-wrap items-center justify-between gap-4">
		<div class="flex items-center gap-3">
			<h1 class="text-2xl font-semibold">{vm.name}</h1>
			<StateBadge state={vm.state} />
		</div>
		<div class="flex items-center gap-3">
			{#if vm.state === 'running'}
				<a
					href={resolve('/vms/[name]/console', { name: vm.name })}
					class="rounded-sm text-sm underline underline-offset-4 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
				>
					Console
				</a>
			{/if}
			<VmActions {vm} />
		</div>
	</div>

	<h2 class="mb-3 text-lg font-semibold">Live use</h2>
	{#if vm.state !== 'running'}
		<p>Live values show while the VM runs.</p>
	{:else if !mine}
		<p role="status">Waiting for the first values…</p>
	{:else}
		<!-- Refreshes every 5 seconds. aria-live stays off: a screen reader
		     would read every refresh. -->
		<dl class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
			<div class="rounded-xl border bg-card p-4">
				<dt class="text-sm text-muted-foreground">CPU</dt>
				<dd class="text-xl font-semibold tabular-nums">
					{mine.cpu_percent === null ? unknown : `${mine.cpu_percent.toFixed(1)} %`}
				</dd>
				<dd class="text-xs text-muted-foreground">
					of {vm.vcpus} vCPU{vm.vcpus === 1 ? '' : 's'}
				</dd>
			</div>
			<div class="rounded-xl border bg-card p-4">
				<dt class="text-sm text-muted-foreground">Memory</dt>
				<dd class="text-xl font-semibold tabular-nums">{memory(mine)}</dd>
			</div>
			<div class="rounded-xl border bg-card p-4">
				<dt class="text-sm text-muted-foreground">Disk</dt>
				<dd class="tabular-nums">Read {rate(mine.disk_read_bps)}</dd>
				<dd class="tabular-nums">Write {rate(mine.disk_write_bps)}</dd>
			</div>
			<div class="rounded-xl border bg-card p-4">
				<dt class="text-sm text-muted-foreground">Network</dt>
				<dd class="tabular-nums">Received {rate(mine.net_rx_bps)}</dd>
				<dd class="tabular-nums">Sent {rate(mine.net_tx_bps)}</dd>
			</div>
		</dl>
		<p class="mt-2 text-xs text-muted-foreground">
			Refreshes every 5 seconds. A dash means that libvirt reports no value yet.
		</p>
	{/if}

	<VmManage {vm} />
{/if}
