## Description

What does this change and why?

## Related issue

Fixes #(issue number)

## Type of change

- [ ] Bug fix (non-breaking change that fixes an issue)
- [ ] New feature (non-breaking change that adds functionality)
- [ ] Breaking change (changes existing behavior)
- [ ] Documentation
- [ ] Refactor / internal (no behavior change)

## Checklist

- [ ] `just check` passes (fmt, clippy with `-D warnings`, tests). The cargo steps need `libvirt-dev`
- [ ] Added or updated tests for the change. Tests use the libvirt test driver (`test:///default`), never a real VM
- [ ] Added a `.changeset/<slug>.md` fragment with a one-line body (or the `no-changelog` label applies).
      For a user-visible change, also updated the docs. `CHANGELOG.md` is generated, never hand-edited
- [ ] The PR title follows Conventional Commits, and the PR targets `main`
- [ ] A new pinned version (tool, action input, container image, toolchain) has its Renovate entry in
      `schubydoo/renovate-config` (`lodger.json`)
- [ ] Kept Lodger's invariants: `unsafe` only in `crates/lodger-virt/src/{events,stats}/ffi.rs`; only `lodger-virt`
      imports `virt`; libvirt stays the source of truth (no VM data in SQLite); XML only from the builders,
      never by joining strings; no subprocess calls; no secret in any log

## Notes for reviewers

Anything that needs extra attention, or manual steps to verify.
