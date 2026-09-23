<script lang="ts">
	import { useQueryClient } from '@tanstack/svelte-query';
	import * as Card from '$lib/components/ui/card';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import { ApiError, keys, problemText, send, type Session } from '$lib/api';

	const client = useQueryClient();

	let username = $state('');
	let password = $state('');
	let problem = $state('');
	let busy = $state(false);

	async function logIn(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		problem = '';
		try {
			const session = await send<Session>('POST', '/api/session', {
				body: { username, password }
			});
			password = '';
			// The app shell sees the session and opens the app.
			client.setQueryData(keys.session, session);
		} catch (e) {
			problem =
				e instanceof ApiError && e.status === 401 ? 'Wrong username or password.' : problemText(e);
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head><title>Log in · Lodger</title></svelte:head>

<Card.Root class="mx-auto mt-12 max-w-sm">
	<Card.Header>
		<Card.Title><h1 class="text-xl font-semibold">Log in to Lodger</h1></Card.Title>
	</Card.Header>
	<Card.Content>
		<form class="grid gap-4" onsubmit={logIn}>
			<div class="grid gap-2">
				<Label for="username">Username</Label>
				<Input id="username" autocomplete="username" required bind:value={username} />
			</div>
			<div class="grid gap-2">
				<Label for="password">Password</Label>
				<Input
					id="password"
					type="password"
					autocomplete="current-password"
					required
					bind:value={password}
				/>
			</div>
			{#if problem}
				<p role="alert" class="text-sm text-destructive">{problem}</p>
			{/if}
			<Button type="submit" disabled={busy}>{busy ? 'Logging in…' : 'Log in'}</Button>
		</form>
	</Card.Content>
</Card.Root>
