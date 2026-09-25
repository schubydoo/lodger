# Lodger web app

The SvelteKit single-page app for Lodger. The Rust binary embeds the static build
from `web/build` (`crates/lodger/src/assets.rs`), so a Lodger install never needs
Node. Node and pnpm are needed only to build and develop this app. `just build` builds
this app first, then the release binary.

- Node: the version in `../.node-version`
- pnpm: the version in the `packageManager` field of `package.json`, through corepack
  (`corepack enable`, or run `corepack pnpm ...`)

```
pnpm install       # install from pnpm-lock.yaml
pnpm run dev       # Vite dev server
pnpm run check     # svelte-check
pnpm run lint      # prettier + eslint
pnpm run test      # vitest
pnpm run build     # static build into build/, with the 200.html fallback
pnpm run test:e2e  # Playwright flows and axe (tests/), see below
```

`pnpm run test:e2e` starts the debug server on libvirt's test driver, so build
both first: `pnpm run build` here and `cargo build -p lodger` in the repo root.
It also needs a Chromium: run `pnpm exec playwright install chromium` once, or
point `LODGER_E2E_CHROMIUM` at an installed Chromium or headless shell.

The build writes `/third-party-notices.txt`. Its Rust section comes from
`web/notices/rust.txt`, which `just notices` generates (it needs cargo-about).
Run it first, or use `just build`, which does both. Without it, the build warns
and the file has no Rust crates. `web/notices.ts` explains the parts.

`pnpm-workspace.yaml` sets pnpm's supply-chain policy. Read its comments before you
add an exception.
