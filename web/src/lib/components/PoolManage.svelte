<!-- Start, stop, autostart, and remove for one pool (PRD F6). Removal lists
     the VMs that have a disk in the pool first, and it needs the pool's name.
     Deleting the pool's volumes is a separate choice. -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { useQueryClient } from '@tanstack/svelte-query';
	import Problem from '$lib/components/Problem.svelte';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { ApiError, changePool, keys, removePool, type PoolDetail, type Session } from '$lib/api';

	let { pool }: { pool: PoolDetail } = $props();

	const client = useQueryClient();
	const csrf = () => client.getQueryData<Session | null>(keys.session)?.csrf_token;

	let busy = $state<'active' | 'autostart' | 'remove' | null>(null);
	let problem = $state<unknown>(null);
	let removing = $state(false);
	let typed = $state('');
	let deleteFiles = $state(false);

	const running = $derived(pool.state === 'running');

	function failed(e: unknown) {
		if (e instanceof ApiError && e.status === 401) client.setQueryData(keys.session, null);
		problem = e;
	}

	async function change(what: 'active' | 'autostart') {
		if (busy) return;
		problem = null;
		busy = what;
		try {
			await changePool(
				pool.uuid,
				what === 'active' ? { active: !running } : { autostart: !pool.autostart },
				{ csrf: csrf() }
			);
		} catch (e) {
			failed(e);
		} finally {
			busy = null;
		}
	}

	async function remove() {
		if (busy || typed !== pool.name) return;
		problem = null;
		busy = 'remove';
		try {
			await removePool(pool.uuid, { confirm: typed, deleteFiles, csrf: csrf() });
			await goto(resolve('/storage'));
		} catch (e) {
			failed(e);
		} finally {
			busy = null;
		}
	}
</script>

<div class="flex flex-wrap items-center gap-2">
	<Button variant="outline" size="sm" disabled={busy !== null} onclick={() => change('active')}>
		{busy === 'active' ? 'Asking…' : running ? 'Stop' : 'Start'}
	</Button>
	<Button variant="outline" size="sm" disabled={busy !== null} onclick={() => change('autostart')}>
		{busy === 'autostart' ? 'Saving…' : pool.autostart ? 'Turn autostart off' : 'Turn autostart on'}
	</Button>
</div>

<section aria-labelledby="remove-{pool.uuid}" class="mt-8">
	<h2 id="remove-{pool.uuid}" class="mb-2 text-lg font-semibold">Remove</h2>
	{#if pool.used_by.length > 0}
		<div role="note" class="mb-3 text-sm">
			<p>These VMs have a disk in {pool.name}. They lose it if its files are deleted:</p>
			<ul class="mt-1 list-disc pl-5">
				{#each pool.used_by as vm (vm)}
					<li>{vm}</li>
				{/each}
			</ul>
		</div>
	{/if}
	{#if removing}
		<div class="flex flex-col gap-3">
			<label class="flex items-center gap-2 text-sm">
				<input type="checkbox" bind:checked={deleteFiles} />
				Also delete every volume in the pool
			</label>
			<div class="flex flex-wrap items-center gap-2">
				<label class="text-sm" for="remove-{pool.uuid}-name">Type {pool.name} to remove it</label>
				<Input
					id="remove-{pool.uuid}-name"
					class="h-8 w-48"
					autocomplete="off"
					spellcheck={false}
					bind:value={typed}
				/>
				<Button
					variant="destructive"
					size="sm"
					disabled={typed !== pool.name || busy !== null}
					onclick={remove}
				>
					{busy === 'remove' ? 'Removing…' : 'Remove'}
				</Button>
				<Button
					variant="outline"
					size="sm"
					onclick={() => {
						removing = false;
						typed = '';
						deleteFiles = false;
					}}
				>
					Cancel
				</Button>
			</div>
		</div>
	{:else}
		<Button
			variant="outline"
			size="sm"
			aria-label="Remove {pool.name}"
			disabled={busy !== null}
			onclick={() => (removing = true)}
		>
			Remove…
		</Button>
	{/if}
</section>
{#if problem !== null}
	<Problem error={problem} class="mt-3" />
{/if}
