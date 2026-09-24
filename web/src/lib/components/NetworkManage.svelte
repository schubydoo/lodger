<!-- Start, stop, autostart, and delete for one network (PRD F8). Delete
     lists the VMs with a NIC on the network first, and it needs the
     network's name. -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { useQueryClient } from '@tanstack/svelte-query';
	import Problem from '$lib/components/Problem.svelte';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import {
		ApiError,
		changeNetwork,
		deleteNetwork,
		keys,
		type NetworkDetail,
		type Session
	} from '$lib/api';

	let { network }: { network: NetworkDetail } = $props();

	const client = useQueryClient();
	const csrf = () => client.getQueryData<Session | null>(keys.session)?.csrf_token;

	let busy = $state<'active' | 'autostart' | 'delete' | null>(null);
	let problem = $state<unknown>(null);
	let deleting = $state(false);
	let typed = $state('');

	function failed(e: unknown) {
		if (e instanceof ApiError && e.status === 401) client.setQueryData(keys.session, null);
		problem = e;
	}

	async function change(what: 'active' | 'autostart') {
		if (busy) return;
		problem = null;
		busy = what;
		try {
			await changeNetwork(
				network.uuid,
				what === 'active' ? { active: !network.active } : { autostart: !network.autostart },
				{ csrf: csrf() }
			);
		} catch (e) {
			failed(e);
		} finally {
			busy = null;
		}
	}

	async function remove() {
		if (busy || typed !== network.name) return;
		problem = null;
		busy = 'delete';
		try {
			await deleteNetwork(network.uuid, { confirm: typed, csrf: csrf() });
			await client.invalidateQueries({ queryKey: keys.networks });
			await goto(resolve('/networks'));
		} catch (e) {
			failed(e);
		} finally {
			busy = null;
		}
	}
</script>

<div class="flex flex-wrap items-center gap-2">
	<Button variant="outline" size="sm" disabled={busy !== null} onclick={() => change('active')}>
		{busy === 'active' ? 'Asking…' : network.active ? 'Stop' : 'Start'}
	</Button>
	<Button variant="outline" size="sm" disabled={busy !== null} onclick={() => change('autostart')}>
		{busy === 'autostart'
			? 'Saving…'
			: network.autostart
				? 'Turn autostart off'
				: 'Turn autostart on'}
	</Button>
</div>

<section aria-labelledby="delete-{network.uuid}" class="mt-8">
	<h2 id="delete-{network.uuid}" class="mb-2 text-lg font-semibold">Delete</h2>
	{#if network.used_by.length > 0}
		<div role="note" class="mb-3 text-sm">
			<p>These VMs have a NIC on {network.name}. It stops working when the network is gone:</p>
			<ul class="mt-1 list-disc pl-5">
				{#each network.used_by as vm (vm)}
					<li>{vm}</li>
				{/each}
			</ul>
		</div>
	{/if}
	{#if deleting}
		<div class="flex flex-wrap items-center gap-2">
			<label class="text-sm" for="delete-{network.uuid}-name"
				>Type {network.name} to delete it</label
			>
			<Input
				id="delete-{network.uuid}-name"
				class="h-8 w-48"
				autocomplete="off"
				spellcheck={false}
				bind:value={typed}
			/>
			<Button
				variant="destructive"
				size="sm"
				disabled={typed !== network.name || busy !== null}
				onclick={remove}
			>
				{busy === 'delete' ? 'Deleting…' : 'Delete'}
			</Button>
			<Button
				variant="outline"
				size="sm"
				onclick={() => {
					deleting = false;
					typed = '';
				}}
			>
				Cancel
			</Button>
		</div>
	{:else}
		<Button
			variant="outline"
			size="sm"
			aria-label="Delete {network.name}"
			disabled={busy !== null}
			onclick={() => (deleting = true)}
		>
			Delete…
		</Button>
	{/if}
</section>
{#if problem !== null}
	<Problem error={problem} class="mt-3" />
{/if}
