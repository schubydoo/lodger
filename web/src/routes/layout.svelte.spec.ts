import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import { createRawSnippet } from 'svelte';
import Layout from './+layout.svelte';
import { host } from '$lib/test/fixtures';

/** A WebSocket stand-in that records each socket the layout opens. */
class FakeSocket {
	static all: FakeSocket[] = [];
	onopen: (() => void) | null = null;
	onmessage: ((e: MessageEvent) => void) | null = null;
	onclose: (() => void) | null = null;
	constructor(public url: string) {
		FakeSocket.all.push(this);
	}
	close() {}
}

// Each socket gets a ticket first. The ticket client has its own tests.
vi.mock('$lib/session', async (actual) => ({
	...(await actual<typeof import('$lib/session')>()),
	nextTicket: vi.fn(async () => 'tk')
}));

const children = createRawSnippet(() => ({ render: () => '<p>page body</p>' }));

beforeEach(() => {
	FakeSocket.all = [];
	vi.stubGlobal('WebSocket', FakeSocket);
	vi.stubGlobal(
		'fetch',
		vi.fn(async () => new Response(JSON.stringify(host)))
	);
});

afterEach(() => vi.unstubAllGlobals());

describe('the layout', () => {
	it('has a skip link, the main navigation, the page, and the notices link', () => {
		render(Layout, { props: { children } });
		expect(screen.getByRole('link', { name: 'Skip to content' })).toHaveAttribute('href', '#main');
		const nav = screen.getByRole('navigation', { name: 'Main' });
		expect(nav).toHaveTextContent('Overview');
		expect(nav).toHaveTextContent('Virtual machines');
		expect(screen.getByText('page body')).toBeInTheDocument();
		expect(screen.getByRole('link', { name: 'Third-party notices' })).toHaveAttribute(
			'href',
			'/third-party-notices.txt'
		);
	});

	it('opens the events socket on the page origin, with a ticket', async () => {
		render(Layout, { props: { children } });
		await vi.waitFor(() => expect(FakeSocket.all).toHaveLength(1));
		expect(FakeSocket.all[0].url).toBe(`ws://${window.location.host}/ws/events?ticket=tk`);
	});

	it('shows the banner when the events socket closes', async () => {
		render(Layout, { props: { children } });
		await vi.waitFor(() => expect(FakeSocket.all).toHaveLength(1));
		FakeSocket.all[0].onclose?.();
		expect(await screen.findByText('Lodger is not reachable')).toBeInTheDocument();
	});
});
