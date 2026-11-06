# The `virt` crate pin

Lodger uses one exact crates.io version of the `virt` crate. This file records which version, why it was a Git commit before, and what the review found. Update it in the same pull request as any change to the `virt` line in `Cargo.toml`.

## Current pin

| Field | Value |
|---|---|
| Source | crates.io, `virt = "=0.5.0"` |
| Crate versions | `virt` 0.5.0 and `virt-sys` 0.3.2, both LGPL-2.1 |
| Upstream commit | `f4ac9211c7eaaca61f93d52d95d8d3c6f806494f` ("release v0.5.0", 2026-10-29) in `https://gitlab.com/libvirt/libvirt-rust.git` |
| Published | 2026-10-29 |
| Reviewed | 2026-11-06 |

The review on 2026-11-06 compared the crates.io files with the Git tree at `f4ac921`. `src/`, `virt-sys/src/`, `virt-sys/build.rs`, and the dependency lists are identical. Lodger used `f4ac921` as a Git pin before, so the move to crates.io changes no code.

## History: the Git pin

From 2026-09-21 to 2026-11-06, Lodger used `virt` from a Git commit on `master`. The crate had no release after v0.4.3 (2025-08-21) for 13 months, but `master` held fixes that Lodger needs. The first pin was `0bd0cbf` ("virt-sys: Regenerate bindings", 2026-08-27). A later update moved it to `f4ac921`, which adds 2 commits: `19390f2` ("virt-sys: Do not generate layout tests") and the release commit. Neither commit changes `src/` or `virt-sys/src/`. Upstream then published `f4ac921` as v0.5.0, so Lodger moved back to crates.io.

## Fixes that Lodger needs

All of these are in 0.5.0, and none is in 0.4.3.

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

## Problems still present in 0.5.0

- When libvirt returns an error, `event_add_handle` and `event_add_timeout` in `src/event.rs` free the caller's `opaque` pointer instead of the box they created. That corrupts memory. Lodger never calls these two functions. It uses libvirt's default event loop and its own event glue in `lodger-virt`.
- Two string macros in `src/lib.rs` still call `CString::new(...).unwrap()`, so a NUL byte in their input still panics. `lodger-core` rejects NUL bytes before any call reaches `virt`.

## Checklist for the next version update

1. List the commits: `git log --format='%h %cs %s' v<old>..v<new>` in a clone of the repository.
2. Read every commit that touches `src/` or `virt-sys/src/`.
3. Make sure that the crates.io files match the Git tag: compare `src/` and `virt-sys/src/` from `~/.cargo/registry/src/` with the clone.
4. Record new fixes, new breaking changes, and the state of the two open problems above.
5. Run `just check` and the libvirt test-driver tests.
6. Update the table at the top of this file, and run `just notices`.
