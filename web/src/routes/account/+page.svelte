<script lang="ts">
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import * as Card from '$lib/components/ui/card';
	import * as Table from '$lib/components/ui/table';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import {
		ApiError,
		fetchAccounts,
		keys,
		problemText,
		send,
		type Account,
		type Session
	} from '$lib/api';

	const client = useQueryClient();
	const accounts = createQuery(() => ({ queryKey: keys.accounts, queryFn: () => fetchAccounts() }));

	/** The CSRF token of the current session. The app shell loaded it. */
	const csrf = () => client.getQueryData<Session | null>(keys.session)?.csrf_token;

	/** A 401 means that the session ended: the app shell then shows the login. */
	function failed(e: unknown): string {
		if (e instanceof ApiError && e.status === 401) client.setQueryData(keys.session, null);
		return problemText(e);
	}

	// Change the own password.
	let current = $state('');
	let fresh = $state('');
	let again = $state('');
	let passwordProblem = $state('');
	let passwordDone = $state('');
	let changing = $state(false);

	async function changePassword(event: SubmitEvent) {
		event.preventDefault();
		passwordProblem = passwordDone = '';
		if (fresh !== again) {
			passwordProblem = 'The two new passwords are not the same.';
			return;
		}
		changing = true;
		try {
			const { ended_sessions } = await send<{ ended_sessions: number }>(
				'POST',
				'/api/account/password',
				{ body: { current_password: current, new_password: fresh }, csrf: csrf() }
			);
			current = fresh = again = '';
			passwordDone =
				ended_sessions === 1
					? 'Password changed. 1 other session ended.'
					: `Password changed. ${ended_sessions} other sessions ended.`;
		} catch (e) {
			passwordProblem = failed(e);
		} finally {
			changing = false;
		}
	}

	// Add an account.
	let newName = $state('');
	let newPassword = $state('');
	let addProblem = $state('');
	let adding = $state(false);

	async function addAccount(event: SubmitEvent) {
		event.preventDefault();
		addProblem = '';
		adding = true;
		try {
			await send('POST', '/api/accounts', {
				body: { username: newName, password: newPassword },
				csrf: csrf()
			});
			newName = newPassword = '';
			await client.invalidateQueries({ queryKey: keys.accounts });
		} catch (e) {
			addProblem = failed(e);
		} finally {
			adding = false;
		}
	}

	// Delete an account: the first click asks, the second deletes.
	let confirming = $state<number | null>(null);
	let deleteProblem = $state('');

	async function deleteAccount(account: Account) {
		deleteProblem = '';
		try {
			await send('DELETE', `/api/accounts/${account.id}`, { csrf: csrf() });
			confirming = null;
			if (account.you) {
				// The own account is gone, and its session with it.
				client.setQueryData(keys.session, null);
				return;
			}
			await client.invalidateQueries({ queryKey: keys.accounts });
		} catch (e) {
			confirming = null;
			deleteProblem = failed(e);
		}
	}
</script>

<svelte:head><title>Account · Lodger</title></svelte:head>

<h1 class="mb-6 text-2xl font-semibold">Account</h1>

<div class="grid gap-6 lg:grid-cols-2">
	<Card.Root>
		<Card.Header>
			<Card.Title><h2>Change your password</h2></Card.Title>
			<Card.Description>Your other sessions end, on every device.</Card.Description>
		</Card.Header>
		<Card.Content>
			<form class="grid gap-4" onsubmit={changePassword}>
				<div class="grid gap-2">
					<Label for="current">Current password</Label>
					<Input
						id="current"
						type="password"
						autocomplete="current-password"
						required
						bind:value={current}
					/>
				</div>
				<div class="grid gap-2">
					<Label for="fresh">New password</Label>
					<Input
						id="fresh"
						type="password"
						autocomplete="new-password"
						aria-describedby="fresh-rule"
						required
						bind:value={fresh}
					/>
					<p id="fresh-rule" class="text-xs text-muted-foreground">
						At least 15 characters, and not a common password.
					</p>
				</div>
				<div class="grid gap-2">
					<Label for="again">New password again</Label>
					<Input
						id="again"
						type="password"
						autocomplete="new-password"
						required
						bind:value={again}
					/>
				</div>
				{#if passwordProblem}
					<p role="alert" class="text-sm text-destructive">{passwordProblem}</p>
				{/if}
				{#if passwordDone}
					<p role="status" class="text-sm">{passwordDone}</p>
				{/if}
				<Button type="submit" disabled={changing}>
					{changing ? 'Changing…' : 'Change the password'}
				</Button>
			</form>
		</Card.Content>
	</Card.Root>

	<Card.Root>
		<Card.Header>
			<Card.Title><h2>Add an account</h2></Card.Title>
			<Card.Description>Every account has full rights.</Card.Description>
		</Card.Header>
		<Card.Content>
			<form class="grid gap-4" onsubmit={addAccount}>
				<div class="grid gap-2">
					<Label for="new-name">Username</Label>
					<Input id="new-name" autocomplete="off" required bind:value={newName} />
				</div>
				<div class="grid gap-2">
					<Label for="new-password">Password</Label>
					<Input
						id="new-password"
						type="password"
						autocomplete="new-password"
						aria-describedby="new-password-rule"
						required
						bind:value={newPassword}
					/>
					<p id="new-password-rule" class="text-xs text-muted-foreground">
						At least 15 characters, and not a common password.
					</p>
				</div>
				{#if addProblem}
					<p role="alert" class="text-sm text-destructive">{addProblem}</p>
				{/if}
				<Button type="submit" disabled={adding}>{adding ? 'Adding…' : 'Add the account'}</Button>
			</form>
		</Card.Content>
	</Card.Root>
</div>

<h2 class="mt-8 mb-3 text-lg font-semibold">Accounts</h2>
{#if accounts.isPending}
	<p>Loading the accounts…</p>
{:else if accounts.isError}
	<p role="alert">Could not load the accounts: {accounts.error.message}</p>
{:else}
	{#if deleteProblem}
		<p role="alert" class="mb-3 text-sm text-destructive">{deleteProblem}</p>
	{/if}
	<Table.Root>
		<Table.Header>
			<Table.Row>
				<Table.Head>Username</Table.Head>
				<Table.Head>Password changed</Table.Head>
				<Table.Head><span class="sr-only">Actions</span></Table.Head>
			</Table.Row>
		</Table.Header>
		<Table.Body>
			{#each accounts.data as account (account.id)}
				<Table.Row>
					<Table.Cell>
						{account.username}
						{#if account.you}<span class="text-muted-foreground"> (you)</span>{/if}
					</Table.Cell>
					<Table.Cell>{new Date(account.password_changed_at).toLocaleString()}</Table.Cell>
					<Table.Cell class="text-right">
						{#if confirming === account.id}
							<span class="mr-2 text-sm">Delete {account.username}?</span>
							<Button variant="destructive" size="sm" onclick={() => deleteAccount(account)}>
								Yes, delete
							</Button>
							<Button variant="outline" size="sm" onclick={() => (confirming = null)}>Keep</Button>
						{:else}
							<Button
								variant="outline"
								size="sm"
								aria-label="Delete {account.username}"
								onclick={() => (confirming = account.id)}
							>
								Delete
							</Button>
						{/if}
					</Table.Cell>
				</Table.Row>
			{/each}
		</Table.Body>
	</Table.Root>
{/if}
