<!-- The app around every page: it sends a visitor to setup or login, and it
     shows the navigation, the connection banner, and the events socket only
     to a logged-in user. -->
<script lang="ts">
	import type { Snippet } from 'svelte';
	import { asset, resolve } from '$app/paths';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import ConnectionBanner from '$lib/components/ConnectionBanner.svelte';
	import { Button } from '$lib/components/ui/button';
	import { fetchSession, fetchSetupOpen, keys, send } from '$lib/api';
	import { connectEvents, eventsUrl } from '$lib/events';
	import { nextTicket, withTicket } from '$lib/session';

	let { children }: { children: Snippet } = $props();

	const client = useQueryClient();
	const session = createQuery(() => ({
		queryKey: keys.session,
		queryFn: () => fetchSession(),
		retry: false
	}));
	const setupOpen = createQuery(() => ({
		queryKey: keys.setup,
		queryFn: () => fetchSetupOpen(),
		retry: false
	}));

	const path = $derived(page.url.pathname);
	const onPublicPage = $derived(path === resolve('/login') || path === resolve('/setup'));
	const user = $derived(session.data ?? null);

	// Where the visitor must go, if not here.
	$effect(() => {
		if (session.isPending || setupOpen.isPending) return;
		const open = setupOpen.data === true;
		if (open && path !== resolve('/setup')) {
			void goto(resolve('/setup'), { replaceState: true });
		} else if (!open && !user && path !== resolve('/login')) {
			void goto(resolve('/login'), { replaceState: true });
		} else if (user && onPublicPage) {
			void goto(resolve('/'), { replaceState: true });
		}
	});

	// Assume open until the first close, so the banner does not flash at start.
	let socketOpen = $state(true);

	// The events socket needs a session, so it runs only while one exists.
	$effect(() => {
		if (!user) return;
		socketOpen = true;
		return connectEvents({
			url: async () => withTicket(eventsUrl(window.location), await nextTicket()),
			client,
			onOpenChange: (open) => (socketOpen = open)
		});
	});

	let loggingOut = $state(false);

	async function logOut() {
		if (!user) return;
		loggingOut = true;
		try {
			await send('DELETE', '/api/session', { csrf: user.csrf_token });
		} catch {
			// The session may be gone already. Either way, it is over here.
		}
		loggingOut = false;
		// Drop every cached answer, so the next user sees nothing of this one.
		client.removeQueries({ predicate: (q) => q.queryKey[0] !== keys.session[0] });
		client.setQueryData(keys.session, null);
	}

	const nav = [
		{ href: resolve('/'), label: 'Overview' },
		{ href: resolve('/vms'), label: 'Virtual machines' },
		{ href: resolve('/storage'), label: 'Storage' },
		{ href: resolve('/networks'), label: 'Networks' },
		{ href: resolve('/account'), label: 'Account' }
	];
</script>

<a
	href="#main"
	class="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:rounded-md focus:bg-background focus:px-3 focus:py-2 focus:ring-2 focus:ring-ring"
>
	Skip to content
</a>
{#if user && !onPublicPage}
	<header class="border-b">
		<nav aria-label="Main" class="mx-auto flex max-w-6xl items-center gap-6 px-4 py-3">
			<span class="font-semibold">Lodger</span>
			<ul class="flex gap-4">
				{#each nav as item (item.href)}
					<li>
						<a
							href={item.href}
							aria-current={path === item.href ? 'page' : undefined}
							class="rounded-sm text-sm text-muted-foreground underline-offset-4 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none aria-[current=page]:font-medium aria-[current=page]:text-foreground aria-[current=page]:underline"
						>
							{item.label}
						</a>
					</li>
				{/each}
			</ul>
			<div class="ml-auto flex items-center gap-3 text-sm">
				<span class="text-muted-foreground">{user.username}</span>
				<Button variant="outline" size="sm" disabled={loggingOut} onclick={logOut}>Log out</Button>
			</div>
		</nav>
	</header>
	<ConnectionBanner {socketOpen} />
{/if}
<main id="main" tabindex="-1" class="mx-auto max-w-6xl px-4 py-6 focus:outline-none">
	{#if onPublicPage || user}
		{@render children()}
	{:else if session.isError}
		<p role="alert">Could not reach Lodger: {session.error.message}</p>
	{:else}
		<p>Loading…</p>
	{/if}
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
