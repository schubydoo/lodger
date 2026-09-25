<!-- Start, stop, autostart, and remove for one pool (PRD F6). Removal lists
     the VMs that have a disk in the pool first, and it needs the pool's name.
     Deleting the pool's volumes is a separate choice. The page's other
     sections go in `children`, between the buttons and Remove, so Remove
     stays last. -->
<script lang="ts">
	import type { Snippet } from 'svelte';
	import { vanishing } from '$lib/vanishing.svelte';
	import { noAutofill } from '$lib/no-autofill';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { useQueryClient } from '@tanstack/svelte-query';
	import Problem from '$lib/components/Problem.svelte';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { ApiError, changePool, keys, removePool, type PoolDetail, type Session } from '$lib/api';

	let { pool, children }: { pool: PoolDetail; children?: Snippet } = $props();

	const client = useQueryClient();
	const csrf = () => client.getQueryData<Session | null>(keys.session)?.csrf_token;

	let busy = $state<'active' | 'autostart' | 'remove' | null>(null);
	// Each error shows next to the control that caused it.
	let changeProblem = $state<unknown>(null);
	let removeProblem = $state<unknown>(null);
	let removing = $state(false);
	let typed = $state('');
	let deleteFiles = $state(false);

	const running = $derived(pool.state === 'running');

	function failed(e: unknown): unknown {
		if (e instanceof ApiError && e.status === 401) client.setQueryData(keys.session, null);
		return e;
	}

	async function change(what: 'active' | 'autostart') {
		if (busy) return;
		changeProblem = removeProblem = null;
		busy = what;
		try {
			await changePool(
				pool.uuid,
				what === 'active' ? { active: !running } : { autostart: !pool.autostart },
				{ csrf: csrf() }
			);
		} catch (e) {
			changeProblem = failed(e);
		} finally {
			busy = null;
		}
	}

	async function remove() {
		if (busy || typed !== pool.name) return;
		changeProblem = removeProblem = null;
		busy = 'remove';
		// The page stops fetching the pool: libvirt's event for the removal would
		// make it ask for the pool again and get 404.
		vanishing.add(pool.uuid);
		try {
			await removePool(pool.uuid, { confirm: typed, deleteFiles, csrf: csrf() });
			// Leave first, then drop the pool's cached detail and refresh the list.
			await goto(resolve('/storage'));
			client.removeQueries({ queryKey: keys.pool(pool.uuid) });
			vanishing.delete(pool.uuid);
			await client.invalidateQueries({ queryKey: keys.pools });
		} catch (e) {
			vanishing.delete(pool.uuid);
			removeProblem = failed(e);
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
{#if changeProblem !== null}
	<Problem error={changeProblem} class="mt-3" />
{/if}

{@render children?.()}

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
					{...noAutofill}
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
	{#if removeProblem !== null}
		<Problem error={removeProblem} class="mt-3" />
	{/if}
</section>
