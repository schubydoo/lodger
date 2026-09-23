// Tickets for the WebSocket upgrades (crates/lodger/src/tickets.rs). Each
// upgrade needs a fresh ticket from POST /api/ws-tickets, which needs the
// session's CSRF token from GET /api/session.

/**
 * Returns a function that fetches one ticket per call. It keeps the CSRF
 * token between calls. After a 403 it fetches the token once more, because a
 * new login replaces the session and its token.
 */
export function ticketSource(fetcher: typeof fetch = fetch): () => Promise<string> {
	let csrf: string | null = null;

	const csrfToken = async (): Promise<string> => {
		const res = await fetcher('/api/session', { headers: { accept: 'application/json' } });
		if (!res.ok) throw new Error(`/api/session answered ${res.status} ${res.statusText}`);
		return ((await res.json()) as { csrf_token: string }).csrf_token;
	};

	const request = async (): Promise<Response> => {
		csrf ??= await csrfToken();
		return fetcher('/api/ws-tickets', { method: 'POST', headers: { 'x-csrf-token': csrf } });
	};

	return async () => {
		let res = await request();
		if (res.status === 403) {
			csrf = null;
			res = await request();
		}
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
