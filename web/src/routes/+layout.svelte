<script lang="ts">
	import './layout.css';
	import favicon from '$lib/assets/favicon.svg';
	import { onMount } from 'svelte';
	import { asset, resolve } from '$app/paths';
	import { page } from '$app/state';
	import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query';
	import ConnectionBanner from '$lib/components/ConnectionBanner.svelte';
	import { connectEvents, eventsUrl } from '$lib/events';
	import { nextTicket, withTicket } from '$lib/session';

	let { children } = $props();

	// The events socket tells the client when data changes, so the cache
	// never goes stale on a timer.
	const client = new QueryClient({
		defaultOptions: { queries: { staleTime: Infinity, retry: 1 } }
	});

	// Assume open until the first close, so the banner does not flash at start.
	let socketOpen = $state(true);

	onMount(() =>
		connectEvents({
			url: async () => withTicket(eventsUrl(window.location), await nextTicket()),
			client,
			onOpenChange: (open) => (socketOpen = open)
		})
	);

	const nav = [
		{ href: resolve('/'), label: 'Overview' },
		{ href: resolve('/vms'), label: 'Virtual machines' }
	];
</script>

<svelte:head><link rel="icon" href={favicon} /></svelte:head>

<QueryClientProvider {client}>
	<a
		href="#main"
		class="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:rounded-md focus:bg-background focus:px-3 focus:py-2 focus:ring-2 focus:ring-ring"
	>
		Skip to content
	</a>
	<header class="border-b">
		<nav aria-label="Main" class="mx-auto flex max-w-6xl items-center gap-6 px-4 py-3">
			<span class="font-semibold">Lodger</span>
			<ul class="flex gap-4">
				{#each nav as item (item.href)}
					<li>
						<a
							href={item.href}
							aria-current={page.url.pathname === item.href ? 'page' : undefined}
							class="rounded-sm text-sm text-muted-foreground underline-offset-4 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none aria-[current=page]:font-medium aria-[current=page]:text-foreground aria-[current=page]:underline"
						>
							{item.label}
						</a>
					</li>
				{/each}
			</ul>
		</nav>
	</header>
	<ConnectionBanner {socketOpen} />
	<main id="main" tabindex="-1" class="mx-auto max-w-6xl px-4 py-6 focus:outline-none">
		{@render children()}
	</main>
	<footer class="mx-auto max-w-6xl border-t px-4 py-4 text-xs text-muted-foreground">
		<!-- A static file, not a route: data-sveltekit-reload makes the browser
		     load it instead of the client router. -->
		<a
			href={asset('/third-party-notices.txt')}
			data-sveltekit-reload
			class="rounded-sm underline underline-offset-4 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
		>
			Third-party notices
		</a>
	</footer>
</QueryClientProvider>
