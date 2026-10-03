# Contributing to corisco-wallet

Thanks for considering a contribution. This repo has two components that
evolve together: the firmware (`firmware-core` + 
`esp32-lilygo-t-display-s3-firmware`) and `corisco-android-app` share one
BLE wire protocol (`firmware-core/src/ble.rs`'s `Request`/`Response` enums
<-> `corisco-android-app/src/postcard.ts`'s encoder), so a protocol change
is expected to touch both sides in the same PR.

## Building each component

- **Firmware**: `firmware-core` and `esp32-lilygo-t-display-s3-firmware`
  are members of one Cargo workspace rooted at the repo root -- see
  [`esp32-lilygo-t-display-s3-firmware/README.md`](esp32-lilygo-t-display-s3-firmware/README.md)
  for the Xtensa toolchain setup, then `cargo build --release` from the
  repo root.
- **`corisco-android-app`**: see [`corisco-android-app/README.md`](corisco-android-app/README.md)
  for Android/Expo setup, then `npx tsc --noEmit` for a quick sanity check.

## Relationship to crypto-core

The firmware's signing logic comes from a separate repo,
[crypto-core](https://github.com/corisco-wallet/crypto-core), pinned to a
release tag in the workspace root `Cargo.toml`'s
`[workspace.dependencies]` table. If your change needs a crypto-core
update: open and merge that PR first, then bump the pin here in a
follow-up PR. A crypto-core change isn't visible in this repo's CI until
that pin is bumped.

### Developing against a local crypto-core

To test unreleased crypto-core changes without editing the committed
manifest, put this in a gitignored `.cargo/patch.toml` and pass it with
`--config`:

```toml
[patch."https://github.com/corisco-wallet/crypto-core"]
corisco-crypto-core = { path = "../crypto-core" }
```

```bash
cargo build --release --config .cargo/patch.toml
```

## Wire protocol changes

New `Request`/`Response` variants must be **appended**, not inserted --
postcard's wire format encodes an enum variant by its declaration-order
discriminant, so inserting one silently breaks compatibility with
whatever's already deployed. Both `ble.rs` and `postcard.ts` need the new
variant in the same PR, appended in the same relative position on each
side.

## Coding conventions

- Comments should explain *why*, not *what* -- if a comment just restates
  what the next line obviously does, or narrates how a bug was found and
  fixed, it doesn't belong. Explain non-obvious invariants and hardware/
  protocol gotchas instead.
- Rust: `cargo fmt`/`cargo clippy` clean. TypeScript: `tsc --noEmit`
  clean.

## Changes that get extra scrutiny

This is a key-custody device. Changes to `ble.rs`'s confirmation-gating
logic (`requires_confirmation`, the `Sign` request path), anything in
`ble-hardware-signer.ts`'s `withSpendContext`/`withoutSpendConfirmation`,
or PIN/seed storage (`firmware-core/src/storage.rs`, `seed_lock.rs` in
crypto-core) get more careful review than a typical PR. If you think
you've found an actual vulnerability, please follow this org's
`SECURITY.md` for private disclosure rather than opening a public issue.

## Review

See `.github/CODEOWNERS` for who reviews what. Branch protection on
`main` requires CI (firmware build + corisco-android-app typecheck) to pass
and at least one approving review before merge.
