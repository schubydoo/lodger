// Helpers for the VNC console page.

/** The console socket URL for VM `uuid` on the page's own origin. */
export function vncUrl(location: Pick<Location, 'protocol' | 'host'>, uuid: string): string {
	const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
	return `${scheme}//${location.host}/ws/vms/${encodeURIComponent(uuid)}/vnc`;
}

export type ConsoleStatus = 'connecting' | 'connected' | 'closed' | 'failed';

/** The text for each status. The status line reads it out. */
export function statusText(status: ConsoleStatus): string {
	switch (status) {
		case 'connecting':
			return 'Connecting to the console…';
		case 'connected':
			return 'Connected. Click the screen to type into the VM.';
		case 'closed':
			return 'The console closed.';
		case 'failed':
			return 'The console could not connect or closed with an error. The VM may have stopped.';
	}
}
