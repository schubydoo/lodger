<!-- Autostart and delete for one VM (PRD F4), on the VM's page. Delete
     removes the VM for good, so it asks for the VM's name first, and it can
     also delete the VM's volumes that no other VM uses. libvirt deletes only
     a shut-off VM. After a delete, the page moves to the VM list, which
     shows what happened to each disk. -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { useQueryClient } from '@tanstack/svelte-query';
	import Problem from '$lib/components/Problem.svelte';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { ApiError, deleteVm, keys, setAutostart, type Session, type Vm } from '$lib/api';
	import { lastDeletion } from '$lib/deletion.svelte';

	let { vm }: { vm: Vm } = $props();

	const client = useQueryClient();
	const csrf = () => client.getQueryData<Session | null>(keys.session)?.csrf_token;

	const canDelete = $derived(vm.state === 'shutoff' || vm.state === 'crashed');

	let busy = $state<'autostart' | 'delete' | null>(null);
	/** The last failure, or `null`. */
	let problem = $state<unknown>(null);
	let deleting = $state(false);
	let typed = $state('');
	let removeVolumes = $state(false);

	function closeDelete() {
		deleting = false;
		typed = '';
		removeVolumes = false;
	}

	// A VM that starts meanwhile closes the delete field.
	$effect(() => {
		if (deleting && !canDelete) closeDelete();
	});

	function failed(e: unknown) {
		// A 401 means that the session ended: the app shell then shows the login.
		if (e instanceof ApiError && e.status === 401) client.setQueryData(keys.session, null);
		problem = e;
	}

	async function toggleAutostart() {
		if (busy) return;
		problem = null;
		busy = 'autostart';
		try {
			await setAutostart(vm.uuid, !vm.autostart, { csrf: csrf() });
		} catch (e) {
			failed(e);
		} finally {
			busy = null;
		}
	}

	async function remove() {
		// jsdom and fast clicks can reach a disabled button: check again.
		if (busy || typed !== vm.name) return;
		problem = null;
		busy = 'delete';
		try {
			const removal = await deleteVm(vm.uuid, { confirm: typed, removeVolumes, csrf: csrf() });
			lastDeletion.current = { name: vm.name, removal };
			await goto(resolve('/vms'));
		} catch (e) {
			failed(e);
		} finally {
			busy = null;
		}
	}
</script>

<section aria-labelledby="settings-{vm.uuid}" class="mt-8">
	<h2 id="settings-{vm.uuid}" class="mb-3 text-lg font-semibold">Settings</h2>
	<div class="flex flex-wrap items-center gap-3">
		<p>
			Autostart is {vm.autostart ? 'on' : 'off'}: libvirt {vm.autostart
				? 'starts'
				: 'does not start'}
			{vm.name} when the host boots.
		</p>
		<Button variant="outline" size="sm" disabled={busy !== null} onclick={toggleAutostart}>
			{busy === 'autostart' ? 'Saving…' : vm.autostart ? 'Turn autostart off' : 'Turn autostart on'}
		</Button>
	</div>

	<h3 class="mt-6 mb-2 font-semibold">Delete</h3>
	{#if !canDelete}
		<p class="text-sm text-muted-foreground">Shut {vm.name} down to delete it.</p>
	{:else if deleting}
		<div class="flex flex-col gap-3">
			<label class="flex items-center gap-2 text-sm">
				<input type="checkbox" bind:checked={removeVolumes} />
				Also delete its volumes that no other VM uses
			</label>
			<div class="flex flex-wrap items-center gap-2">
				<label class="text-sm" for="delete-{vm.uuid}">Type {vm.name} to delete it</label>
				<Input
					id="delete-{vm.uuid}"
					class="h-8 w-40"
					autocomplete="off"
					spellcheck={false}
					bind:value={typed}
				/>
				<Button
					variant="destructive"
					size="sm"
					disabled={typed !== vm.name || busy !== null}
					onclick={remove}
				>
					{busy === 'delete' ? 'Deleting…' : 'Delete'}
				</Button>
				<Button variant="outline" size="sm" onclick={closeDelete}>Cancel</Button>
			</div>
		</div>
	{:else}
		<Button
			variant="outline"
			size="sm"
			aria-label="Delete {vm.name}"
			disabled={busy !== null}
			onclick={() => (deleting = true)}
		>
			Delete…
		</Button>
	{/if}
	{#if problem !== null}
		<Problem error={problem} class="mt-2" />
	{/if}
</section>
