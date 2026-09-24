// The result of the last VM delete. The VM page moves to the VM list after a
// delete, and the list shows this once, until the user dismisses it.

import type { Removal } from './api';

export const lastDeletion = $state<{ current: { name: string; removal: Removal } | null }>({
	current: null
});
