import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { createRawSnippet } from 'svelte';
import Layout from './+layout.svelte';
import { host } from '$lib/test/fixtures';

/** A WebSocket stand-in that records each socket the layout opens. */
class FakeSocket {
	static all: FakeSocket[] = [];
	onopen: (() => void) | null = null;
	onmessage: ((e: MessageEvent) => void) | null = null;
	onclose: (() => void) | null = null;
	closed = false;
	constructor(public url: string) {
		FakeSocket.all.push(this);
	}
	close() {
		this.closed = true;
		this.onclose?.();
	}
}

// Each socket gets a ticket first. The ticket client has its own tests.
vi.mock('$lib/session', async (actual) => ({
	...(await actual<typeof import('$lib/session')>()),
	nextTicket: vi.fn(async () => 'tk')
}));

const nav = vi.hoisted(() => ({ goto: vi.fn(async () => {}), path: '/' }));
vi.mock('$app/navigation', () => ({ goto: nav.goto }));
vi.mock('$app/state', () => ({
	page: {
		get url() {
			return new URL(`http://lodger.test${nav.path}`);
		}
	}
}));

const children = createRawSnippet(() => ({ render: () => '<p>page body</p>' }));

/** The server's answers: setup open or not, and a session or none. */
function serve(state: { setupOpen?: boolean; loggedIn?: boolean }) {
	const calls: { path: string; init?: RequestInit }[] = [];
	const answer = (body: unknown, status = 200) =>
		new Response(body === null ? '' : JSON.stringify(body), { status });
	vi.stubGlobal(
		'fetch',
		vi.fn(async (path: string, init?: RequestInit) => {
			calls.push({ path, init });
			if (path === '/api/setup')
				return answer(state.setupOpen ? { required: true } : null, state.setupOpen ? 200 : 404);
			if (path === '/api/session' && init?.method === 'DELETE') return answer(null, 204);
			if (path === '/api/session')
				return state.loggedIn
					? answer({ username: 'admin', csrf_token: 'csrf1' })
					: answer({ error: 'log in first' }, 401);
			if (path === '/api/host') return answer(host);
			return answer({ error: 'not here' }, 404);
		})
	);
	return calls;
}

beforeEach(() => {
	FakeSocket.all = [];
	vi.stubGlobal('WebSocket', FakeSocket);
	nav.path = '/';
	nav.goto.mockClear();
});

afterEach(() => vi.unstubAllGlobals());

describe('the layout', () => {
	it('shows the navigation, the page, and the notices link to a logged-in user', async () => {
		serve({ loggedIn: true });
		render(Layout, { props: { children } });
		const menu = await screen.findByRole('navigation', { name: 'Main' });
		expect(screen.getByRole('link', { name: 'Skip to content' })).toHaveAttribute('href', '#main');
		expect(menu).toHaveTextContent('Overview');
		expect(menu).toHaveTextContent('Virtual machines');
		expect(menu).toHaveTextContent('Account');
		expect(menu).toHaveTextContent('admin');
		expect(screen.getByText('page body')).toBeInTheDocument();
		expect(screen.getByRole('link', { name: 'Third-party notices' })).toHaveAttribute(
			'href',
			'/third-party-notices.txt'
		);
	});

	it('opens the events socket with a ticket, and shows the banner when it closes', async () => {
		serve({ loggedIn: true });
		render(Layout, { props: { children } });
		await vi.waitFor(() => expect(FakeSocket.all).toHaveLength(1));
		expect(FakeSocket.all[0].url).toBe(`ws://${window.location.host}/ws/events?ticket=tk`);
		FakeSocket.all[0].onclose?.();
		expect(await screen.findByText('Lodger is not reachable')).toBeInTheDocument();
	});

	it('sends a visitor without a session to the login page, and opens no socket', async () => {
		serve({ loggedIn: false });
		render(Layout, { props: { children } });
		await vi.waitFor(() => expect(nav.goto).toHaveBeenCalledWith('/login', { replaceState: true }));
		expect(screen.queryByText('page body')).not.toBeInTheDocument();
		expect(screen.queryByRole('navigation')).not.toBeInTheDocument();
		expect(FakeSocket.all).toHaveLength(0);
	});

	it('shows the login page itself without the navigation', async () => {
		serve({ loggedIn: false });
		nav.path = '/login';
		render(Layout, { props: { children } });
		expect(await screen.findByText('page body')).toBeInTheDocument();
		await new Promise((resolve) => setTimeout(resolve, 20));
		expect(nav.goto).not.toHaveBeenCalled();
		expect(screen.queryByRole('navigation')).not.toBeInTheDocument();
	});

	it('sends everyone to setup while no account exists', async () => {
		serve({ setupOpen: true });
		render(Layout, { props: { children } });
		await vi.waitFor(() => expect(nav.goto).toHaveBeenCalledWith('/setup', { replaceState: true }));
	});

	it('sends a logged-in user away from the login page', async () => {
		serve({ loggedIn: true });
		nav.path = '/login';
		render(Layout, { props: { children } });
		await vi.waitFor(() => expect(nav.goto).toHaveBeenCalledWith('/', { replaceState: true }));
	});

	it('logs out with the CSRF token, closes the socket, and goes to the login page', async () => {
		const calls = serve({ loggedIn: true });
		render(Layout, { props: { children } });
		await vi.waitFor(() => expect(FakeSocket.all).toHaveLength(1));
		await fireEvent.click(screen.getByRole('button', { name: 'Log out' }));
		await vi.waitFor(() => expect(nav.goto).toHaveBeenCalledWith('/login', { replaceState: true }));
		const logout = calls.find((c) => c.init?.method === 'DELETE');
		expect(logout?.path).toBe('/api/session');
		expect((logout?.init?.headers as Record<string, string>)['x-csrf-token']).toBe('csrf1');
		expect(FakeSocket.all[0].closed).toBe(true);
		expect(screen.queryByRole('navigation')).not.toBeInTheDocument();
	});
});
