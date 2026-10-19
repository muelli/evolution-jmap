<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Equivalent or out-of-scope mutants in `jmap-config`

## `src/defaults.rs`

`from_identity`'s body survives replaced with `Default::default()` only as
an `Unviable` mutant: `Account` has no `Default` impl, so it does not
compile. No action needed.

## `src/complete.rs`

`<impl std::error::Error for Incomplete>::source`'s body survives replaced
with `Some(Box::leak(Box::new(Default::default())))` only as an `Unviable`
mutant: the boxed value has no concrete type to default-construct, so it
does not compile. No action needed.

## `src/oauth2_service.rs`

Five mutants, all `Unviable` (none compile), no action needed:
- `<impl ObjectSubclass for Service>::interfaces`'s body replaced with
  `vec![Default::default()]`: `InterfaceDecl` has no `Default` impl.
- `Verifiers::redeem`'s body replaced with `Some(Zeroizing::new())`,
  `Some(Zeroizing::from_iter([String::new()]))`,
  `Some(Zeroizing::from_iter(["xyzzy".into()]))` or
  `Some(Zeroizing::from("xyzzy".into()))`: each is a type or arity mismatch
  against `Zeroizing<String>`.
