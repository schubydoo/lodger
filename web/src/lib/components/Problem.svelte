<!-- One failure, as a sentence. A known libvirt error also shows its cause,
     its fix, and the commands of the fix, which the user runs on the host
     (PRD R10). An unknown error shows only its own text. -->
<script lang="ts">
	import { explanationOf, problemText } from '$lib/api';

	let { error, class: className = '' }: { error: unknown; class?: string } = $props();

	const known = $derived(explanationOf(error));
</script>

<div role="alert" class="text-sm {className}">
	<p class="text-destructive">{problemText(error)}</p>
	{#if known}
		<p class="mt-1">{known.cause}</p>
		<p class="mt-1">{known.fix}</p>
		{#if known.commands.length > 0}
			<pre
				aria-label="Commands to run on the host"
				class="mt-2 overflow-x-auto rounded-md bg-muted p-2 text-left text-xs"><code
					>{known.commands.join('\n')}</code
				></pre>
		{/if}
	{/if}
</div>
