import { defineConfig, devices } from '@playwright/test';

// End-to-end and accessibility tests (Task 3.11, TAD 9.3). They run against
// the debug server on libvirt's test driver. `test:///default` is one shared
// state in the server process, so the flows run in order in one worker.
const port = 8471;

export default defineConfig({
	testDir: '.',
	testMatch: '**/*.e2e.ts',
	fullyParallel: false,
	workers: 1,
	forbidOnly: !!process.env.CI,
	retries: 0,
	reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
	use: {
		baseURL: `http://127.0.0.1:${port}`,
		trace: 'retain-on-failure',
		...devices['Desktop Chrome'],
		// A local run can point at an installed Chromium; CI installs the one
		// that matches this Playwright version.
		launchOptions: process.env.LODGER_E2E_CHROMIUM
			? { executablePath: process.env.LODGER_E2E_CHROMIUM }
			: {}
	},
	webServer: {
		command: `sh serve.sh ${port}`,
		url: `http://127.0.0.1:${port}/api/health`,
		reuseExistingServer: false,
		timeout: 60_000
	}
});
