<!-- Start, Shut down, and Force off for one VM (PRD F4). The buttons ask
     libvirt and then wait: the new state arrives through the events socket,
     which refreshes the list. Force off loses unsaved data in the guest, so
     it asks for the VM's name first, and its button stays disabled until the
     typed name matches. -->
<script lang="ts">
	import { useQueryClient } from '@tanstack/svelte-query';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import {
		ApiError,
		keys,
		problemText,
		vmAction,
		type Session,
		type Vm,
		type VmAction
	} from '$lib/api';

	let { vm }: { vm: Vm } = $props();

	const client = useQueryClient();

	/** Libvirt can start only a VM that is not active. */
	const canStart = $derived(vm.state === 'shutoff' || vm.state === 'crashed');
	/** Force off works on every active state. */
	const canForceOff = $derived(
		['running', 'blocked', 'paused', 'shutting_down'].includes(vm.state)
	);

	let busy = $state<VmAction | null>(null);
	let problem = $state('');
	let forcing = $state(false);
	let typed = $state('');
	/** The state in which a shutdown was requested, until it changes. */
	let shutdownAskedIn = $state<Vm['state'] | null>(null);

	// A new state from libvirt ends a pending shutdown request.
	$effect(() => {
		if (shutdownAskedIn !== null && vm.state !== shutdownAskedIn) shutdownAskedIn = null;
	});

	// A VM that stops for another reason closes the Force off field.
	$effect(() => {
		if (forcing && !canForceOff) {
			forcing = false;
			typed = '';
		}
	});

	async function run(action: VmAction) {
		// The buttons are disabled meanwhile. This return also stops a
		// second click that arrives before the page updates.
		if (busy) return;
		if (action === 'force-off' && typed !== vm.name) return;
		problem = '';
		busy = action;
		// The state before the request: the event can refresh the list
		// before the answer arrives.
		const before = vm.state;
		try {
			await vmAction(vm.uuid, action, {
				confirm: action === 'force-off' ? typed : undefined,
				csrf: client.getQueryData<Session | null>(keys.session)?.csrf_token
			});
			if (action === 'shutdown') shutdownAskedIn = before;
			if (action === 'force-off') {
				forcing = false;
				typed = '';
			}
		} catch (e) {
			// A 401 means that the session ended: the app shell then shows
			// the login.
			if (e instanceof ApiError && e.status === 401) client.setQueryData(keys.session, null);
			problem = problemText(e);
		} finally {
			busy = null;
		}
	}
</script>

<div class="flex flex-wrap items-center justify-end gap-2">
	{#if forcing}
		<label class="text-sm" for="force-off-{vm.uuid}">Type {vm.name} to force it off</label>
		<Input
			id="force-off-{vm.uuid}"
			class="h-8 w-40"
			autocomplete="off"
			spellcheck={false}
			bind:value={typed}
		/>
		<Button
			variant="destructive"
			size="sm"
			disabled={typed !== vm.name || busy !== null}
			onclick={() => run('force-off')}
		>
			{busy === 'force-off' ? 'Forcing off…' : 'Force off'}
		</Button>
		<Button
			variant="outline"
			size="sm"
			onclick={() => {
				forcing = false;
				typed = '';
			}}
		>
			Cancel
		</Button>
	{:else}
		{#if canStart}
			<Button
				variant="outline"
				size="sm"
				aria-label="Start {vm.name}"
				disabled={busy !== null}
				onclick={() => run('start')}
			>
				{busy === 'start' ? 'Starting…' : 'Start'}
			</Button>
		{/if}
		{#if vm.state === 'running'}
			<Button
				variant="outline"
				size="sm"
				aria-label="Shut down {vm.name}"
				disabled={busy !== null}
				onclick={() => run('shutdown')}
			>
				{busy === 'shutdown' ? 'Asking…' : 'Shut down'}
			</Button>
		{/if}
		{#if canForceOff}
			<Button
				variant="outline"
				size="sm"
				aria-label="Force off {vm.name}"
				disabled={busy !== null}
				onclick={() => (forcing = true)}
			>
				Force off
			</Button>
		{/if}
	{/if}
</div>
{#if shutdownAskedIn !== null}
	<p role="status" class="mt-1 text-right text-sm text-muted-foreground">
		Shutdown requested. {vm.name} stops when its guest finishes.
	</p>
{/if}
{#if problem}
	<p role="alert" class="mt-1 text-right text-sm text-destructive">{problem}</p>
{/if}
