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
```

`pnpm-workspace.yaml` sets pnpm's supply-chain policy. Read its comments before you
add an exception.
