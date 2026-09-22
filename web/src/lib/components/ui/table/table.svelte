<script lang="ts">
	import { cn, type WithElementRef } from '$lib/utils.js';
	import type { HTMLTableAttributes } from 'svelte/elements';

	let {
		ref = $bindable(null),
		class: className,
		children,
		label,
		...restProps
	}: WithElementRef<HTMLTableAttributes> & {
		/** Lodger change: with a label, the scroll container is a named region
		 *  that takes focus, so a keyboard can scroll a wide table (WCAG 2.1.1). */
		label?: string;
	} = $props();
</script>

<!-- A focusable scroll region is the WAI pattern for a wide table, so the
     noninteractive-tabindex rule does not apply here. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div
	data-slot="table-container"
	class="relative w-full overflow-x-auto focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
	role={label ? 'region' : undefined}
	aria-label={label}
	tabindex={label ? 0 : undefined}
>
	<table
		bind:this={ref}
		data-slot="table"
		class={cn('w-full caption-bottom text-sm', className)}
		{...restProps}
	>
		{@render children?.()}
	</table>
</div>
