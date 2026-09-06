# Native connection setup

Open `target/debug/openloops-ui.exe` in File Explorer. Connection setup takes
place in the window; neither credentials nor model selection require a terminal.
The same native window now includes [conversation review and reviewed To Do
reminders](inbox-review.md). It is a foreground preview; the Outlook add-in and
unattended background service remain on the implementation roadmap.

## Using the window

1. Enter the Application (client) ID from the development app registration.
   Enter any Outlook Group primary addresses in the Groups box, one per line or
   separated by commas. Mailboxes opened through **Shared with me** or **Open
   another mailbox** belong in the separate optional shared-mailbox box.
2. Click **Sign in & check inboxes** and complete Microsoft sign-in in the system
   browser. The window reports personal, Group, and shared-mailbox access
   separately. These read-only checks do not load message subjects or bodies.
3. Paste an Ollama API key. It is hidden by default; **Show key** reveals it.
   Click **Load cloud models**, choose a model, then **Test selected model**.
   The list comes from Ollama Cloud, with its exact model labels preserved.
   DeepSeek Flash is suggested when listed, including dated versions.
4. To switch, select another model and test it. No automatic fallback occurs.
   A failed test distinguishes authentication, rate limiting, account balance,
   HTTP server/request errors, timeouts, and invalid analysis output. Upstream
   response text is never shown or logged.

The client ID, Group and shared-mailbox addresses, Ollama API key, and selected
model save automatically when changed and restore on launch. They are stored
together as one versioned record in the current user's Windows Credential Manager,
with **Local** persistence (this computer, across logins). No settings file or
plaintext key is written to the checkout. The key is hidden again on every launch.
The model list can be refreshed with **Load cloud models**; the selected model is
remembered without automatically contacting Ollama at startup.

**Save settings** retries a failed save. **Reload saved settings** replaces the
current inputs with the saved copy. **Forget saved settings** removes the saved
record and clears the fields. If loading fails, automatic saving pauses so empty
inputs cannot overwrite an unreadable record. An explicit Save can replace it.
The UI reports save failures and leaves the previous record intact. Windows limits
the complete encoded record to 2,560 bytes; an oversized inbox list produces an
error rather than truncating settings.

The Microsoft check still discards its token and sign-in is required again; access
check results and the Show key toggle are not saved. The separate
[Review inboxes tab](inbox-review.md) automatically analyzes recent mail from
configured inboxes when the user clicks Scan inboxes.
Reminder scheduling and task creation remain unimplemented. The Ollama
test sends a fixed content-free request. Successful generation confirms that
request's validity, not a model's extraction quality.

The app registration is still a developer setup prerequisite. Ordinary user
onboarding will need a publisher-owned multitenant registration configured in
the distributed application; users should not each need to register an app.
See [Microsoft connection](live-connection.md) and [Ollama Cloud](ollama-cloud.md)
for permission and data-flow details.

## Development

Build the executable once from a development checkout:

```powershell
cargo build -p openloops-desktop --bin openloops-ui --features native-ui --locked
```

The native window uses eframe with its generic persistence disabled. A narrow
`settings.rs` module uses pinned `keyring-core` and `windows-native-keyring-store`
dependencies to access only the `OpenLoops/Setup/v1` generic credential, with
credential search disabled. Network requests run on
a background thread, inputs are disabled while a request is pending, and the
window remains responsive with elapsed-time feedback. Settings operations are
serialized on the UI thread. No setup values or provider responses are logged.
Serialized settings and API keys use zeroizing buffers without claiming removal
of all UI, allocator, TLS, or operating-system copies.

For visual checks on empty fields only, `ui-screenshot` builds a noninteractive
window that saves `openloops-setup-empty.png` in the operating-system temporary
directory and exits. This uses eframe's screenshot event before buffer swapping.
The fields cannot be edited in that build, and real settings are never loaded or
saved. Normal `native-ui` builds do not include screenshot saving. UI unit tests
also disable the production store; the Windows integration test uses an isolated
synthetic credential, verifies it from a fresh process, and deletes it afterward.

This optional interface does not pass a release gate or change the existing
Phase 0 network-dependency restrictions. It is a local development executable,
not a packaged or signed installer.
