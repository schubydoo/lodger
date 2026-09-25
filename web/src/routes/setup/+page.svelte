<script lang="ts">
	import { useQueryClient } from '@tanstack/svelte-query';
	import { noAutofill } from '$lib/no-autofill';
	import * as Card from '$lib/components/ui/card';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import { ApiError, keys, problemText, send, type Session } from '$lib/api';

	const client = useQueryClient();

	let token = $state('');
	let username = $state('');
	let password = $state('');
	let confirm = $state('');
	let problem = $state('');
	let busy = $state(false);

	async function claim(event: SubmitEvent) {
		event.preventDefault();
		problem = '';
		if (password !== confirm) {
			problem = 'The two passwords are not the same.';
			return;
		}
		busy = true;
		// Once an account exists, setup is closed for good. Then the app
		// shell must leave this page, even when the login below fails: to the
		// app with a session, or else to the login page.
		let closed = false;
		try {
			await send('POST', '/api/setup', { body: { token, username, password } });
			closed = true;
			const session = await send<Session>('POST', '/api/session', {
				body: { username, password }
			});
			password = confirm = token = '';
			client.setQueryData(keys.session, session);
		} catch (e) {
			// 404: another tab or another operator finished setup first.
			if (e instanceof ApiError && e.status === 404) closed = true;
			problem = problemText(e);
		} finally {
			busy = false;
			if (closed) client.setQueryData(keys.setup, false);
		}
	}
</script>

<svelte:head><title>Set up · Lodger</title></svelte:head>

<Card.Root class="mx-auto mt-12 max-w-md">
	<Card.Header>
		<Card.Title><h1 class="text-xl font-semibold">Set up Lodger</h1></Card.Title>
		<Card.Description>
			Create the first account. The setup token is in the service log: run
			<code>journalctl -u lodger</code> on the host.
		</Card.Description>
	</Card.Header>
	<Card.Content>
		<form class="grid gap-4" onsubmit={claim}>
			<div class="grid gap-2">
				<Label for="token">Setup token</Label>
				<Input id="token" {...noAutofill} spellcheck={false} required bind:value={token} />
			</div>
			<div class="grid gap-2">
				<Label for="username">Username</Label>
				<Input id="username" autocomplete="username" required bind:value={username} />
			</div>
			<div class="grid gap-2">
				<Label for="password">Password</Label>
				<Input
					id="password"
					type="password"
					autocomplete="new-password"
					aria-describedby="password-rule"
					required
					bind:value={password}
				/>
				<p id="password-rule" class="text-xs text-muted-foreground">
					At least 15 characters. A few words in a row work well.
				</p>
			</div>
			<div class="grid gap-2">
				<Label for="confirm">Password again</Label>
				<Input
					id="confirm"
					type="password"
					autocomplete="new-password"
					required
					bind:value={confirm}
				/>
			</div>
			{#if problem}
				<p role="alert" class="text-sm text-destructive">{problem}</p>
			{/if}
			<Button type="submit" disabled={busy}>{busy ? 'Creating…' : 'Create the account'}</Button>
		</form>
	</Card.Content>
</Card.Root>
