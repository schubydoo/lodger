<!-- Test helper: renders a component inside a QueryClientProvider, because
     svelte-query keeps its client under a private context key. -->
<script lang="ts">
	import type { Component } from 'svelte';
	import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query';

	let {
		client,
		component: Inner,
		props
	}: {
		client: QueryClient;
		// Any component: the test passes props that match it.
		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		component: Component<any>;
		props: Record<string, unknown>;
	} = $props();
</script>

<QueryClientProvider {client}>
	<Inner {...props} />
</QueryClientProvider>
