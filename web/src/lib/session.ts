// Tickets for the WebSocket upgrades (crates/lodger/src/tickets.rs). Each
// upgrade needs a fresh ticket from POST /api/ws-tickets, which needs the
// session's CSRF token from GET /api/session.

/**
 * Returns a function that fetches one ticket per call. It reads the CSRF
 * token each time, because a logout and a new login replace it: a kept token
 * would fail once after every login.
 */
export function ticketSource(fetcher: typeof fetch = fetch): () => Promise<string> {
	return async () => {
		const session = await fetcher('/api/session', { headers: { accept: 'application/json' } });
		if (!session.ok) {
			throw new Error(`/api/session answered ${session.status} ${session.statusText}`);
		}
		const { csrf_token: csrf } = (await session.json()) as { csrf_token: string };
		const res = await fetcher('/api/ws-tickets', {
			method: 'POST',
			headers: { 'x-csrf-token': csrf }
		});
		if (!res.ok) throw new Error(`/api/ws-tickets answered ${res.status} ${res.statusText}`);
		return ((await res.json()) as { ticket: string }).ticket;
	};
}

/** The ticket source that the app shares. */
export const nextTicket = ticketSource();

/** `url` with `ticket` in its query. */
export function withTicket(url: string, ticket: string): string {
	return `${url}?ticket=${encodeURIComponent(ticket)}`;
}
