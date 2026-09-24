<!-- The New pool form (PRD F6 and flow 4.4). A directory pool needs a
     folder. An NFS pool needs the server and the export; Lodger mounts it on
     /var/lib/libvirt/pools/<name> unless the user names a folder. Autostart
     is on unless the user turns it off. Lodger checks the input again, and
     libvirt defines, builds, and starts the pool in one call. -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { useQueryClient } from '@tanstack/svelte-query';
	import Problem from '$lib/components/Problem.svelte';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import { ApiError, createPool, keys, type NewPool, type Session } from '$lib/api';

	const client = useQueryClient();

	let kind = $state<'dir' | 'nfs'>('dir');
	let name = $state('');
	let path = $state('');
	let host = $state('');
	let exportPath = $state('');
	let autostart = $state(true);
	let busy = $state(false);
	let problem = $state<unknown>(null);

	const ready = $derived(
		name.trim() !== '' &&
			(kind === 'dir' ? path.trim() !== '' : host.trim() !== '' && exportPath.trim() !== '')
	);

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (busy || !ready) return;
		problem = null;
		busy = true;
		const pool: NewPool =
			kind === 'dir'
				? { kind, name: name.trim(), path: path.trim(), autostart }
				: {
						kind,
						name: name.trim(),
						host: host.trim(),
						export: exportPath.trim(),
						...(path.trim() ? { path: path.trim() } : {}),
						autostart
					};
		try {
			await createPool(pool, {
				csrf: client.getQueryData<Session | null>(keys.session)?.csrf_token
			});
			await goto(resolve('/storage/[name]', { name: pool.name }));
		} catch (e) {
			if (e instanceof ApiError && e.status === 401) client.setQueryData(keys.session, null);
			problem = e;
		} finally {
			busy = false;
		}
	}
</script>

<form class="flex max-w-xl flex-col gap-4" onsubmit={submit}>
	<fieldset class="flex gap-4">
		<legend class="mb-1 text-sm font-medium">Kind</legend>
		<label class="flex items-center gap-2 text-sm">
			<input type="radio" name="kind" value="dir" bind:group={kind} /> Folder on this host
		</label>
		<label class="flex items-center gap-2 text-sm">
			<input type="radio" name="kind" value="nfs" bind:group={kind} /> NFS share
		</label>
	</fieldset>
	<div class="flex flex-col gap-1">
		<Label for="pool-name">Name</Label>
		<Input id="pool-name" autocomplete="off" spellcheck={false} bind:value={name} />
	</div>
	{#if kind === 'nfs'}
		<div class="flex flex-col gap-1">
			<Label for="pool-host">NFS server</Label>
			<Input id="pool-host" autocomplete="off" spellcheck={false} bind:value={host} />
		</div>
		<div class="flex flex-col gap-1">
			<Label for="pool-export">Export path</Label>
			<Input
				id="pool-export"
				placeholder="/volume1/vm"
				autocomplete="off"
				spellcheck={false}
				bind:value={exportPath}
			/>
		</div>
	{/if}
	<div class="flex flex-col gap-1">
		<Label for="pool-path">{kind === 'dir' ? 'Folder' : 'Mount folder (optional)'}</Label>
		<Input
			id="pool-path"
			placeholder={kind === 'dir'
				? '/var/lib/libvirt/images'
				: `/var/lib/libvirt/pools/${name.trim() || 'name'}`}
			autocomplete="off"
			spellcheck={false}
			bind:value={path}
		/>
	</div>
	<label class="flex items-center gap-2 text-sm">
		<input type="checkbox" bind:checked={autostart} />
		Start the pool when the host boots
	</label>
	<div>
		<Button type="submit" disabled={!ready || busy}>
			{busy ? 'Creating…' : 'Create pool'}
		</Button>
	</div>
	{#if problem !== null}
		<Problem error={problem} />
	{/if}
</form>
