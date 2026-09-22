<script lang="ts">
	import {
		CircleQuestionMark,
		LoaderCircle,
		Moon,
		Pause,
		Play,
		Square,
		TriangleAlert
	} from '@lucide/svelte';
	import type { VmState } from '$lib/api';
	import { stateLook, type StateIcon } from '$lib/vm-state';
	import { cn } from '$lib/utils';

	let { state }: { state: VmState } = $props();

	const icons = {
		play: Play,
		pause: Pause,
		square: Square,
		loader: LoaderCircle,
		alert: TriangleAlert,
		moon: Moon,
		help: CircleQuestionMark
	} satisfies Record<StateIcon, unknown>;

	const look = $derived(stateLook(state));
	const Icon = $derived(icons[look.icon]);
</script>

<!-- The label carries the meaning. The icon repeats it, and it is hidden
     from screen readers so they do not read it twice. -->
<span class={cn('inline-flex items-center gap-1.5 font-medium', look.tone)}>
	<Icon class="size-4 shrink-0" aria-hidden="true" />
	<span>{look.label}</span>
</span>
