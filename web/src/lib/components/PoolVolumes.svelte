<!-- The volumes of one running pool (PRD F7): a table with the VMs that use
     each volume, a delete that needs the volume's name, and the New volume
     form. A volume that a VM or a qcow2 overlay uses has no delete button, and
     the server refuses its delete too. A create or a delete sends a pool event, which refreshes
     this list and the pool's sizes. -->
<script lang="ts">
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import Problem from '$lib/components/Problem.svelte';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import * as Table from '$lib/components/ui/table';
	import {
		ApiError,
		createVolume,
		deleteVolume,
		fetchVolumes,
		formatBytes,
		keys,
		type PoolDetail,
		type Session,
		type Volume
	} from '$lib/api';

	let { pool }: { pool: PoolDetail } = $props();

	/** The VMs and the overlays that use a volume: while any exists, the volume stays. */
	const users = (v: Volume) => [...v.used_by, ...v.backing_for.map((o) => `volume ${o}`)];

	const client = useQueryClient();
	const csrf = () => client.getQueryData<Session | null>(keys.session)?.csrf_token;
	const volumes = createQuery(() => ({
		queryKey: keys.volumes(pool.uuid),
		queryFn: () => fetchVolumes(pool.uuid)
	}));

	let name = $state('');
	let format = $state<'qcow2' | 'raw'>('qcow2');
	let sizeGib = $state<number | null>(20);
	let busy = $state<string | null>(null);
	let problem = $state<unknown>(null);
	/** The volume whose delete waits for its typed name. */
	let deleting = $state<string | null>(null);
	let typed = $state('');

	const ready = $derived(name.trim() !== '' && sizeGib !== null && sizeGib > 0);

	function failed(e: unknown) {
		if (e instanceof ApiError && e.status === 401) client.setQueryData(keys.session, null);
		problem = e;
	}

	async function refresh() {
		await client.invalidateQueries({ queryKey: keys.pool(pool.uuid) });
	}

	async function create(event: SubmitEvent) {
		event.preventDefault();
		if (busy || !ready || sizeGib === null) return;
		problem = null;
		busy = 'create';
		try {
			await createVolume(
				pool.uuid,
				{ name: name.trim(), format, capacity_bytes: Math.round(sizeGib * 1024 ** 3) },
				{ csrf: csrf() }
			);
			name = '';
			await refresh();
		} catch (e) {
			failed(e);
		} finally {
			busy = null;
		}
	}

	async function remove(volume: string) {
		if (busy || typed !== volume) return;
		problem = null;
		busy = volume;
		try {
			await deleteVolume(pool.uuid, volume, { csrf: csrf() });
			deleting = null;
			typed = '';
			await refresh();
		} catch (e) {
			failed(e);
		} finally {
			busy = null;
		}
	}
</script>

<section aria-labelledby="volumes-{pool.uuid}" class="mt-8">
	<h2 id="volumes-{pool.uuid}" class="mb-2 text-lg font-semibold">Volumes</h2>
	{#if volumes.isPending}
		<p>Loading the volumes…</p>
	{:else if volumes.isError}
		<p role="alert">Could not load the volumes: {volumes.error.message}</p>
	{:else if volumes.data.length === 0}
		<p class="text-sm">{pool.name} has no volumes.</p>
	{:else}
		<Table.Root label="Volumes of {pool.name}">
			<Table.Caption class="sr-only">{volumes.data.length} volumes in {pool.name}</Table.Caption>
			<Table.Header>
				<Table.Row>
					<Table.Head scope="col">Name</Table.Head>
					<Table.Head scope="col">Format</Table.Head>
					<Table.Head scope="col" class="text-right">Size</Table.Head>
					<Table.Head scope="col" class="text-right">On disk</Table.Head>
					<Table.Head scope="col">Used by</Table.Head>
					<Table.Head scope="col"><span class="sr-only">Actions</span></Table.Head>
				</Table.Row>
			</Table.Header>
			<Table.Body>
				{#each volumes.data as volume (volume.name)}
					<Table.Row>
						<Table.Cell class="font-medium break-all">{volume.name}</Table.Cell>
						<Table.Cell>{volume.format ?? '–'}</Table.Cell>
						<Table.Cell class="text-right tabular-nums"
							>{formatBytes(volume.capacity_bytes)}</Table.Cell
						>
						<Table.Cell class="text-right tabular-nums">
							{formatBytes(volume.allocation_bytes)}
						</Table.Cell>
						<Table.Cell>{users(volume).length > 0 ? users(volume).join(', ') : '–'}</Table.Cell>
						<Table.Cell>
							{#if users(volume).length > 0}
								<span class="text-sm text-muted-foreground">In use</span>
							{:else if deleting === volume.name}
								<div class="flex flex-wrap items-center gap-2">
									<label class="text-sm" for="delete-{volume.name}">
										Type {volume.name} to delete it
									</label>
									<Input
										id="delete-{volume.name}"
										class="h-8 w-48"
										autocomplete="off"
										spellcheck={false}
										bind:value={typed}
									/>
									<Button
										variant="destructive"
										size="sm"
										disabled={typed !== volume.name || busy !== null}
										onclick={() => remove(volume.name)}
									>
										{busy === volume.name ? 'Deleting…' : 'Delete'}
									</Button>
									<Button
										variant="outline"
										size="sm"
										onclick={() => {
											deleting = null;
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
									aria-label="Delete {volume.name}"
									disabled={busy !== null}
									onclick={() => {
										deleting = volume.name;
										typed = '';
									}}
								>
									Delete…
								</Button>
							{/if}
						</Table.Cell>
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
	{/if}

	<h3 class="mt-6 mb-3 font-semibold">New volume</h3>
	<form class="flex max-w-xl flex-col gap-4" onsubmit={create}>
		<div class="flex flex-col gap-1">
			<Label for="volume-name">Name</Label>
			<Input
				id="volume-name"
				placeholder="disk1.qcow2"
				autocomplete="off"
				spellcheck={false}
				bind:value={name}
			/>
		</div>
		<fieldset class="flex gap-4">
			<legend class="mb-1 text-sm font-medium">Format</legend>
			<label class="flex items-center gap-2 text-sm">
				<input type="radio" name="volume-format" value="qcow2" bind:group={format} /> qcow2
			</label>
			<label class="flex items-center gap-2 text-sm">
				<input type="radio" name="volume-format" value="raw" bind:group={format} /> raw
			</label>
		</fieldset>
		<div class="flex flex-col gap-1">
			<Label for="volume-size">Size (GiB)</Label>
			<Input id="volume-size" type="number" min="0" step="any" class="w-32" bind:value={sizeGib} />
		</div>
		<div>
			<Button type="submit" disabled={!ready || busy !== null}>
				{busy === 'create' ? 'Creating…' : 'Create volume'}
			</Button>
		</div>
	</form>
	{#if problem !== null}
		<Problem error={problem} class="mt-3" />
	{/if}
</section>
