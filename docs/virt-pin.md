# The `virt` crate pin

Lodger uses the `virt` crate from a Git commit, not from crates.io. This file records why, which commit, and what the review found. Update it in the same pull request as any change to the pin in `Cargo.toml`.

## Current pin

| Field | Value |
|---|---|
| Repository | `https://gitlab.com/libvirt/libvirt-rust.git` |
| Branch | `master` |
| Commit | `0bd0cbffce4dd914e76d74e4de480bf95fabdc98` ("virt-sys: Regenerate bindings", 2026-08-27) |
| Crate versions at that commit | `virt` 0.4.3 and `virt-sys` 0.3.2, both LGPL-2.1 |
| Base release | v0.4.3 (2025-08-21) |
| Commits since the base release | 106 |
| Reviewed | 2026-09-21, for the workspace scaffold |

## Why a Git pin

The crate had no release in 13 months, but `master` holds fixes that Lodger needs. Schuby chose the pin on 2026-09-21. When upstream publishes a release that contains this commit, move back to crates.io. Renovate watches for that release.

## Fixes that Lodger needs

- `f72baaa` "Add missing refs on object back pointers". Before this fix, methods that return a parent object, such as the connection of a domain, did not take their own reference. Dropping the connection then underflowed the reference count, which can cause a use-after-free.
- `663f88d` "do not unwrap potential Err when creating CStrings". If a string contains a NUL byte, most methods now return an error instead of a panic. 0.4.3 had 28 such `unwrap()` calls in `domain.rs` alone.
- `08a3663` "Fix: crash in interface_addresses() on null hwaddr". `Interface::hwaddr` is now `Option<String>`. Feature F17 (VM IP addresses) needs this call.
- `d20bb32` "implement the Drop trait for Connect objects". When its last owner drops it, a connection now closes.
- `151e9f6` "Abort on missing library in non-docs.rs builds". A missing `libvirt-dev` now fails the build with a clear error instead of at link time.

## Changes that break code written for 0.4.3

- The `get_` prefix is gone from most methods, for example `get_name()` is now `name()` (commits `3bac0d8` to `c1cc09e`).
- `as_ptr()` is now `unsafe`, and `free()` is private (`4c61194`, `ac77715`).
- Domain state, state reason, error domain, and error number are Rust enums now (`0eb7d9d`, `17e1eec`, `3e13e6e`).
- Object wrappers no longer use `Option<>` inside (`58387d9`).
- Several setters return `()` instead of a count or a `bool` (`6ddbef9`, `6fababc`).

The Phase 0 spike code targets 0.4.3, so it needs these renames before any of it moves into Lodger.

## Problems still present at this commit

- When libvirt returns an error, `event_add_handle` and `event_add_timeout` in `src/event.rs` free the caller's `opaque` pointer instead of the box they created. That corrupts memory. Lodger never calls these two functions. It uses libvirt's default event loop and its own event glue in `lodger-virt`.
- Two string macros in `src/lib.rs` still call `CString::new(...).unwrap()`, so a NUL byte in their input still panics. `lodger-core` rejects NUL bytes before any call reaches `virt`.

## Checklist for the next pin update

1. List the commits: `git log --format='%h %cs %s' <old>..<new>` in a clone of the repository.
2. Read every commit that touches `src/` or `virt-sys/src/`.
3. Record new fixes, new breaking changes, and the state of the two open problems above.
4. Run `just check` and the libvirt test-driver tests.
5. Update the table at the top of this file.
