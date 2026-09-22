<script lang="ts">
	import { createQuery } from '@tanstack/svelte-query';
	import { IconPlugConnectedX } from '@tabler/icons-svelte';
	import * as Alert from '$lib/components/ui/alert';
	import { fetchHost, keys } from '$lib/api';

	/** `false` while the events socket to the Lodger server is closed. */
	let { socketOpen }: { socketOpen: boolean } = $props();

	const host = createQuery(() => ({ queryKey: keys.host, queryFn: () => fetchHost() }));

	const problem = $derived.by(() => {
		if (!socketOpen) {
			return {
				title: 'Lodger is not reachable',
				detail:
					'The page lost its connection to the Lodger server. It tries again every few seconds.'
			};
		}
		const conn = host.data?.connection;
		if (conn && conn.state !== 'connected') {
			return {
				title: conn.state === 'connecting' ? 'Connecting to libvirt' : 'libvirt is not connected',
				detail:
					(conn.error ? `${conn.error}. ` : '') +
					'Lodger tries again every 5 seconds. The data on this page can be out of date.'
			};
		}
		return null;
	});
</script>

<!-- role="alert" makes screen readers announce the banner when it appears.
     The region stays in the page, so it exists before its content does. -->
<div role="alert">
	{#if problem}
		<!-- role={null}: the wrapper is the one alert region, not this box. -->
		<Alert.Root variant="destructive" role={null} class="rounded-none border-x-0 border-t-0">
			<IconPlugConnectedX aria-hidden="true" />
			<Alert.Title>{problem.title}</Alert.Title>
			<Alert.Description>{problem.detail}</Alert.Description>
		</Alert.Root>
	{/if}
</div>
