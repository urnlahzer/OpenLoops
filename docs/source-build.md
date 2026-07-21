# Phase 0 source-build skeleton

**Status:** credential-free synthetic skeleton only; no product capability or gate is enabled.

P0-WI-04 establishes one reproducible Windows build path without implementing
identity, Graph, persistence, inference, an add-in bridge, or a deployable add-in
manifest. The only runnable behavior is a fixed content-free synthetic marker.

## Pinned prerequisites

- Windows 11 x64 validation target from P0-MATRIX-001.
- Rust 1.97.1, `x86_64-pc-windows-msvc`, minimal profile, with Clippy and rustfmt.
- Microsoft C++ Build Tools for the x64 MSVC linker.
- Node.js 24.18.0 LTS and its bundled npm 11.16.0.

The repository pins Rust in `rust-toolchain.toml`, Node in `.node-version`, and
JavaScript dependencies in `package-lock.json`. Install prerequisites from their
official distributions, open a new shell, and run:

```powershell
pwsh ./tools/source-build.ps1
```

The script validates exact versions before restoring locked public dependencies.
It does not install software, accept credentials, read local environment files,
or contact Microsoft Graph or a model provider. Do not place registration values,
tokens, provider keys, mailbox data, or identifiers on a command line or in the
repository. BYO registration configuration remains future gated work.

The build fails closed when repository or ancestor npm/Cargo configuration,
registry credentials, proxies, compiler wrappers, target overrides, or build
flags are present. It gives npm empty temporary user/global configuration and an
exact public registry, and gives Cargo an empty temporary home. These temporary
directories are removed on success or failure without printing configuration
values.

Successful output is exactly the fixed synthetic status
`OPENLOOPS_SYNTHETIC_SMOKE_OK`, followed by a content-free checker result. Build,
generated contract, dependency, and test output remains ignored under `target/`,
`node_modules/`, and `outlook-addin/.generated/`. Source maps are disabled.
The npm dry run is closed to npm's unavoidable sanitized `LICENSE` and
`package.json` metadata only; no application payload is publishable.

## Boundaries

- The domain and contracts layers depend on no adapter.
- Application depends only on domain and contracts.
- Graph, inference, and persistence placeholders depend inward and always report
  unavailable.
- Desktop composition depends on the inward layers and emits only the synthetic
  marker when every adapter, capability, and gate is disabled.
- The TypeScript project supplies compile-time Office.js types only. It has no
  manifest, runtime Office CDN load, mailbox permission, bridge, or durable state.

This work item completes no product acceptance criterion and passes no identity,
Graph, add-in, state, model, privacy, security-audit, or release gate.
