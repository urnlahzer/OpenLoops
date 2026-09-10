# ADR-014: Desktop UI toolkit

- Status: Accepted
- Date: 2026-09-09

## Context

The companion needs a Windows-first native shell, an accessibility tree, and a declarative component structure that follows the Fluent redesign. API keys must remain in Rust-owned state rather than crossing into a browser document or webview process.

## Decision

Use Slint 1.17.1 with its Fluent style, native winit backend, and FemtoVG renderer. The desktop application uses no webview. Provider keys remain in the Rust `AppModel` and are exposed only to the password control needed for editing; they are never logged.

Open fixed external links with `opener`. Setup links are limited to the hard-coded Microsoft Entra and provider key URLs. Message links are permitted only for HTTPS URLs on `outlook.office.com` and `outlook.office365.com`.

The Sources screen includes a visible “Made with Slint” attribution. Slint is used under its royalty-free desktop application licence.

## Consequences

The UI compiles from checked-in `.slint` files at build time and keeps the existing Rust worker/channel model. Generated Slint Rust requires a package-local `unsafe_code` lint exception; hand-written executable and build-script code continue to forbid unsafe code. A unit test reads every hand-written `src/**/*.rs` file in the crate at test time and fails if the token `unsafe` appears outside a comment or string literal, so that package-level allowance stays confined to the generated Slint module rather than spreading into hand-written code.

The title bar shows "Not signed in" until the Graph layer exposes a display name; `ConnectionReport` carries no account identifiers by design, so `AccountDisplay::signed_in` exists and is tested but is not yet reachable from the live connection.

`Tokens.font-family` is the static `"Segoe UI"`, not `"Segoe UI Variable"`. The variable family is installed on Windows only as its variable font file; this build's FemtoVG/fontdb renderer does not drive that file's weight axis reliably, so a 600-weight request against it can render as a synthetic or plain-regular face instead of the installed Semibold. The static family's own Semibold face is always used for weight 600. `Tokens.mono-font-family` is likewise the static `"Consolas"` rather than `"Cascadia Mono"`, which is not installed on every Windows machine this app targets; Slint resolves one family name with no fallback list, so the token holds exactly the face that is actually present.

## Rejected alternative

Dioxus with WebView2 would port the HTML reference closely, but it adds a browser-engine process boundary and the risk of placing credentials in DOM or JavaScript state. That broader secret-handling and rendering surface is unnecessary for this native companion.
