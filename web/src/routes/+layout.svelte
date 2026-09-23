<script lang="ts">
	import './layout.css';
	import favicon from '$lib/assets/favicon.svg';
	import { QueryCache, QueryClient, QueryClientProvider } from '@tanstack/svelte-query';
	import AppShell from '$lib/components/AppShell.svelte';
	import { ApiError, keys } from '$lib/api';

	let { children } = $props();

	// The events socket tells the client when data changes, so the cache
	// never goes stale on a timer. A 401 from any query means that the
	// session ended, for example after 60 idle minutes: forget the user, and
	// the app shell sends the visitor to the login page.
	const client: QueryClient = new QueryClient({
		defaultOptions: { queries: { staleTime: Infinity, retry: 1 } },
		queryCache: new QueryCache({
			onError: (error) => {
				if (error instanceof ApiError && error.status === 401) {
					client.setQueryData(keys.session, null);
				}
			}
		})
	});
</script>

<svelte:head><link rel="icon" href={favicon} /></svelte:head>

<QueryClientProvider {client}>
	<AppShell {children} />
</QueryClientProvider>
