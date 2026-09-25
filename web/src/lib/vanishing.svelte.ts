// The UUIDs of the pools and networks that a delete is removing now. Their
// detail pages stop fetching, so an event that arrives during the delete does
// not ask for the object again and get 404.

import { SvelteSet } from 'svelte/reactivity';

export const vanishing = new SvelteSet<string>();
