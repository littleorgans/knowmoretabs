# Native messaging as a second tab source: decision report

This document answers the `live-tabs` future row in `slices.toml`. It is the
companion to `docs/research/encrypted-sessions.md`, which established that
Chromium is mid-migration to encrypted session storage, that we will not
decrypt it, and that `save` will therefore start refusing on some unannounced
Chrome update.

Every mechanism claim below is tagged:

- **[documented]** — official vendor documentation (Chrome for Developers,
  MDN, Microsoft Learn, Apple Developer).
- **[source]** — read out of Chromium `HEAD` on
  `chromium.googlesource.com`, 2026-09-21.
- **[measured]** — produced by the throwaway prototype in `/tmp/nmproto`
  during this investigation; output is quoted inline.
- **[inferred]** — follows from a documented rule or a source-level rule, but
  was not itself confirmed on a primary source. Treat as a lead, not a fact.

Nothing was installed into a browser. No browser profile was read or written.
The prototype ran entirely in `/tmp` against synthetic data.

---

## Executive answer

**Build it as a second source. Never make it the only source. It is three
slices, and the third one is not code.**

Two corrections to the proposal as it reached this desk, both of which change
the design:

1. **`history` was the wrong permission, and so was the reasoning behind it.**
   We need open tabs, not visited URLs. `chrome.tabs.query({})` and
   `chrome.tabGroups.query({})` are the right calls. The permission bill is
   smaller than feared but not free: `tabs.query()` needs **no permission at
   all** to call, and returns tab id, window, index, `groupId`, pinned,
   active, audible, muted, discarded, `lastAccessed` and the rest with no
   prompt — but `url`, `title`, `pendingUrl` and `favIconUrl` are gated, and
   those four are the entire product. Getting them costs either the `"tabs"`
   permission, whose install prompt reads **"Read your browsing history."**,
   or host permissions, whose prompt is worse. There is no version of this
   that installs silently. [documented]

2. **The 1 MB cap is on the wrong direction to hurt us.** Chrome's cap of
   1 MB applies to messages *from* the host *to* Chrome. Messages from the
   extension *to* the host are capped at 64 MiB. Our payload — the tabs —
   travels extension → host, in the generous direction. 300 tabs measured at
   **140 KB**. The 1 MB ceiling binds only on what the host says back, which
   is a status line. [documented] [source] [measured]

And the answer to the question the project actually cares about: **native
messaging removes the clock from "what is open right now" and does absolutely
nothing for "what was open when Chrome died."** The second half is why this
project exists, and it has no answer that does not involve reading a session
file off disk.

---

## 1. The mechanism, concretely

### 1.1 What "native messaging" is

Chrome starts the native host as a **child process** and talks to it over the
host's `stdin`/`stdout`. The host is an ordinary executable. There is no
socket, no port, no daemon, no IPC library. [documented]

> "Chrome starts each native messaging host in a separate process and
> communicates with it using standard input (`stdin`) and standard output
> (`stdout`)." — [Native messaging](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging)

Two APIs, with different process lifetimes:

| API | Process lifetime |
|---|---|
| `chrome.runtime.connectNative(name)` | One host process, kept running until the `Port` is destroyed. |
| `chrome.runtime.sendNativeMessage(name, msg, cb)` | A **fresh process per message**. The host's first message is the reply; every later message is discarded. |

[documented]

For our shape — "here are 300 tabs, tell me you saved them" —
`sendNativeMessage` is the better fit: one process, one message, one reply,
process exits. It also sidesteps the service-worker lifetime problem in §2.4.

### 1.2 The extension, and what it actually costs in permissions

Manifest V3, service worker, no content scripts, no host permissions:

```json
{
  "manifest_version": 3,
  "name": "knowmoretabs",
  "version": "1.0.0",
  "minimum_chrome_version": "121",
  "description": "Sends your open tabs to the knowmoretabs archive on this machine.",
  "background": { "service_worker": "sw.js" },
  "action": { "default_title": "Save a snapshot" },
  "permissions": ["nativeMessaging", "tabs", "tabGroups", "alarms"],
  "key": "<pinned public key — see §5.3>"
}
```

`minimum_chrome_version: 121` because `Tab.lastAccessed` landed in Chrome 121
and it is the field that makes a live snapshot comparable to the `last_active`
we already parse out of SNSS. [documented]

The whole payload, in about fifteen lines of service worker:

```js
const [tabs, groups, windows] = await Promise.all([
  chrome.tabs.query({}),
  chrome.tabGroups.query({}),
  chrome.windows.getAll({}),
]);
chrome.runtime.sendNativeMessage("com.knowmoretabs.host",
  { v: 1, cmd: "save", captured_at: Date.now(), tabs, groups, windows },
  (reply) => { /* chrome.runtime.lastError, then reply.status */ });
```

**Permission bill, itemised.** This is a product decision, so here is the
whole invoice. Warning strings are quoted from Chrome's
[permissions list](https://developer.chrome.com/docs/extensions/reference/permissions-list).
[documented]

| Permission | Needed for | Install-time warning |
|---|---|---|
| *(none)* | `tabs.query()` itself; tab id, `windowId`, `index`, `groupId`, `pinned`, `active`, `audible`, `mutedInfo`, `discarded`, `autoDiscardable`, `lastAccessed`, `status`, `incognito`, `openerTabId`, `frozen`, `highlighted` | — |
| `nativeMessaging` | Talking to our binary at all | "Communicate with cooperating native applications." |
| `tabs` | **`url`, `title`, `pendingUrl`, `favIconUrl`** — nothing else | **"Read your browsing history."** |
| `tabGroups` | Group *title*, *colour*, *collapsed* via `tabGroups.query()` | "View and manage your tab groups." |
| `alarms` | Scheduling a periodic push (see §2.3) | — |
| `sessions` | `sessions.getRecentlyClosed()` | Combined with `tabs`: "Read your browsing history on **all your signed-in devices**." |

Three findings worth acting on:

- **Group *membership* is free.** `Tab.groupId` is ungated (Chrome 88+), so
  even with no `tabGroups` permission we can reconstruct which tabs share a
  group. Only the group's *name and colour* need the permission, and the
  warning it produces ("View and manage your tab groups") is the mildest on
  the list. Keep it. [documented]
- **`tabs` is unavoidable and is the scary one.** The alternative — host
  permissions such as `<all_urls>` — produces a strictly worse prompt and,
  per the Web Store review docs, strictly more review scrutiny. Take the
  `tabs` permission and write an honest sentence about it in the listing.
  [documented]
- **`sessions` is not worth it.** It upgrades the single prompt from "Read
  your browsing history" to "Read your browsing history on all your
  signed-in devices" — the word *devices* in a prompt for a tool whose
  entire pitch is "no sync, no accounts, no cloud" is a self-inflicted
  wound. And what it buys is thin: `getRecentlyClosed()` is capped at
  `MAX_SESSION_RESULTS = 25` entries and covers only *recently* closed items,
  not a prior session. **Recommendation: do not request `sessions`.**
  [documented]

### 1.3 The native messaging host manifest

A separate JSON file, unrelated to the extension manifest. Five keys, all
documented, all validated by
[`native_messaging_host_manifest.cc`](https://chromium.googlesource.com/chromium/src/+/HEAD/chrome/browser/extensions/api/messaging/native_messaging_host_manifest.cc)
[source]:

```json
{
  "name": "com.knowmoretabs.host",
  "description": "knowmoretabs archive writer",
  "path": "/Users/you/.local/bin/knowmoretabs",
  "type": "stdio",
  "allowed_origins": ["chrome-extension://aaaabbbbccccddddeeeeffffgggghhhh/"]
}
```

| Key | Rule | Evidence |
|---|---|---|
| `name` | Only `[a-z0-9._]`. Cannot start or end with a dot; a dot cannot follow a dot. Checked character by character by `IsValidName()`, not by regex. The file on disk must be named `<name>.json`. | [documented] [source] |
| `description` | Must exist and be non-empty, else `"Invalid value for description."` | [source] |
| `path` | **Absolute on macOS and Linux.** May be relative to the manifest's directory on Windows. The host starts with its working directory set to the directory containing the binary. | [documented] |
| `type` | `"stdio"` is the only accepted value — `"// stdio is the only host type that's currently supported."` | [documented] [source] |
| `allowed_origins` | A list of strings, each parsed as a `URLPattern` with the `SCHEME_EXTENSION` mask. Patterns where `match_all_urls()` or `match_subdomains()` is true are **rejected** as "too broad". No wildcards. | [documented] [source] |

The optional `supports_native_initiated_connections` boolean also exists, but
it is gated behind the `kOnConnectNative` feature and is a dead end for us —
see §2.3. [source]

### 1.4 Where the manifest must be installed

The generalising rule, read out of
[`chrome/common/chrome_paths.cc`](https://chromium.googlesource.com/chromium/src/+/HEAD/chrome/common/chrome_paths.cc)
[source]:

- `DIR_USER_NATIVE_MESSAGING` = **`DIR_USER_DATA` + `NativeMessagingHosts`**.
  One `Append`, no per-platform branching. Anything that moves the user-data
  directory — including `--user-data-dir` — moves this with it.
- `DIR_NATIVE_MESSAGING` (system-wide) is a **hard-coded absolute path**
  chosen by platform and branding. It is never derived from the profile.
- The whole block is guarded by
  `BUILDFLAG(IS_LINUX) || IS_CHROMEOS || IS_MAC || IS_ANDROID`. **Windows is
  absent.** On Windows there is no user-data-dir directory to write to; there
  is only the registry.

That rule is worth more to us than any path table, because it means
`platform.rs` already computes the answer. `BrowserSpec::user_data_dirs()`
returns the user-data directory for all seven browsers on all three
platforms; the native messaging directory is that path plus one component.

**macOS, per-user** — `<user-data-dir>/NativeMessagingHosts/com.knowmoretabs.host.json`:

| Browser | Path under `~` |
|---|---|
| chrome | `Library/Application Support/Google/Chrome/NativeMessagingHosts/` [documented] |
| chrome-beta | `Library/Application Support/Google/Chrome Beta/NativeMessagingHosts/` [inferred] |
| chrome-canary | `Library/Application Support/Google/Chrome Canary/NativeMessagingHosts/` [inferred] |
| chromium | `Library/Application Support/Chromium/NativeMessagingHosts/` [documented] |
| brave | `Library/Application Support/BraveSoftware/Brave-Browser/NativeMessagingHosts/` [inferred] |
| edge | `Library/Application Support/Microsoft Edge/NativeMessagingHosts/` [documented] |
| vivaldi | `Library/Application Support/Vivaldi/NativeMessagingHosts/` [inferred] |

Edge's own documentation adds a channel suffix for non-stable channels:
`~/Library/Application Support/Microsoft Edge {Canary,Dev,Beta}/NativeMessagingHosts/`.
[documented]

**Linux, per-user** — same rule, `~/.config/<browser>/NativeMessagingHosts/`:
`google-chrome`, `google-chrome-beta`, `google-chrome-canary`, `chromium`,
`BraveSoftware/Brave-Browser`, `microsoft-edge`, `vivaldi`. Chrome, Chromium
and Edge are [documented]; Brave and Vivaldi are [inferred] from the
`DIR_USER_DATA` rule and corroborated by third-party installers such as
[`bukubrow-host`](https://github.com/samhh/bukubrow-host), which ships
`--install-brave` / `--install-vivaldi` / `--install-chromium` /
`--install-edge` flags precisely because each browser has its own directory.

**System-wide** — hard-coded, needs root, not something a personal tool should
write to:

| | macOS | Linux |
|---|---|---|
| Chrome | `/Library/Google/Chrome/NativeMessagingHosts` | `/etc/opt/chrome/native-messaging-hosts` |
| Chrome for Testing | `/Library/Google/ChromeForTesting/NativeMessagingHosts` | `/etc/opt/chrome_for_testing/native-messaging-hosts` |
| Chromium | `/Library/Application Support/Chromium/NativeMessagingHosts` | `/etc/chromium/native-messaging-hosts` |
| Edge | `/Library/Microsoft/Edge/NativeMessagingHosts` | `/etc/opt/edge/native-messaging-hosts` |

Chrome and Chromium rows are quoted from `chrome_paths.cc` [source]; Edge from
Microsoft Learn [documented]. Note that before Chrome 146, Chrome for Testing
shared Chrome's paths. [documented]

**Windows — registry, not a file path.** The manifest may live anywhere on
disk; a registry key whose *default* (unnamed) value is the manifest's full
path is what registers it. [documented]

```
HKEY_CURRENT_USER\SOFTWARE\Google\Chrome\NativeMessagingHosts\com.knowmoretabs.host
HKEY_LOCAL_MACHINE\SOFTWARE\Google\Chrome\NativeMessagingHosts\com.knowmoretabs.host
```

```console
REG ADD "HKCU\Software\Google\Chrome\NativeMessagingHosts\com.knowmoretabs.host" ^
  /ve /t REG_SZ /d "C:\path\to\nmh-manifest.json" /f
```

The key name must equal the manifest's `name`. Chrome queries the 32-bit
registry first, then the 64-bit registry. [documented]

Edge documents its full search order, and it ends with a fallback to
Chromium's and Chrome's keys — Edge will use a Chrome-registered host if it
has none of its own, and **stops at the first key it finds**, which means an
extension published to both stores must list *both* extension ids in
`allowed_origins` of the single manifest that gets found. [documented]

```
HKCU\SOFTWARE\Microsoft\Edge\NativeMessagingHosts\
HKCU\SOFTWARE\Chromium\NativeMessagingHosts\
HKCU\SOFTWARE\Google\Chrome\NativeMessagingHosts\
HKLM\SOFTWARE\WOW6432Node\{Microsoft\Edge,Chromium,Google\Chrome}\NativeMessagingHosts\
HKLM\SOFTWARE\{Microsoft\Edge,Chromium,Google\Chrome}\NativeMessagingHosts\
```

Brave's Windows key is
`HKCU\Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\<name>`
[inferred]; Vivaldi's is `HKCU\Software\Vivaldi\NativeMessagingHosts\<name>`
[inferred]. Both need confirming on a real Windows box before slice code
depends on them.

### 1.5 How the extension id is bound to the host

Two directions, and both must hold:

- **Host → extension.** `allowed_origins` is a `URLPatternSet` built with
  `URLPattern::SCHEME_EXTENSION`, and patterns that match all URLs or match
  subdomains are rejected outright. So the binding is to an **exact extension
  id**, by construction, with no way to write a wildcard. [source]
- **Extension → host.** Chrome passes the caller's origin to the host as
  **`argv[1]`**, in the form `chrome-extension://<id>/`. On Windows it also
  passes `--parent-window=<decimal handle>`, which "will be 0 if the calling
  context is a service worker" — which ours always is. [documented]

The extension id itself is the first 128 bits of the SHA-256 of the DER
public key, hex-encoded and then mapped `0-f` → `a-p`, giving the familiar
32-character all-lowercase id. [inferred] The practical consequence is
documented and is the thing that matters: an *unpacked* extension gets an id
derived from its directory path unless you pin the `key` manifest field, at
which point "the extension will use the same ID" everywhere.
[[key](https://developer.chrome.com/docs/extensions/reference/manifest/key)]
[documented] Without a pinned `key`, `allowed_origins` cannot be written by an
installer at all, because the id is not knowable until the user loads the
extension on their own machine.

### 1.6 The wire protocol

> "each message is serialized using JSON, UTF-8 encoded and is preceded with
> 32-bit message length in native byte order." [documented]

Confirmed in source. `OnMessage()` allocates `json.size() + kMessageHeaderSize`
where `kMessageHeaderSize = 4` ("Message header contains 4-byte integer size
of the message"), narrows the length with `base::checked_cast<uint32_t>`, and
`memcpy`s the raw four bytes of that integer into the front of the buffer,
guarded by a `static_assert` that `sizeof(uint32_t) == kMessageHeaderSize`.
The reader `reinterpret_cast`s the buffer start to `const uint32_t*` and
dereferences it. **Neither path performs any byte-order conversion.** The wire
format is host-native endianness — little-endian on every platform we target.
[source]

**Prototype.** `/tmp/nmproto/frame.py` and `/tmp/nmproto/host.rs` implement
both ends and round-trip against each other. [measured]

```
native byte order: little
struct.calcsize('@I') = 4
round trip: {'v': 1, 'cmd': 'tabs'} {'v': 1, 'cmd': 'bye'} trailing bytes: 0
first frame hex: 14 00 00 00 7b 22 76 22
big-endian prefix for len=18: 00 00 00 12 -> reader sees length 301989888
```

That last line is the failure mode worth remembering: a host that writes the
length big-endian does not produce a garbled message, it produces a claimed
length of 301,989,888 bytes and an immediate channel teardown. This is why
`to_ne_bytes` / `from_ne_bytes` and not `to_be_bytes`.

The Rust side is about thirty-five lines of `std`, no dependency:

```rust
let n = u32::from_ne_bytes(len) as usize;      // native order, per the docs
if n > 64 * 1024 * 1024 {                      // bound before allocating
    return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
}
let mut body = vec![0u8; n];
r.read_exact(&mut body)?;
```

Exercised end to end against a Python driver feeding two frames: [measured]

```
stderr: frame 1: 20 bytes
        frame 2: 3206 bytes
reply: {"ok":20}
reply: {"ok":3206}
```

Two non-negotiable rules fall out:

- **`stdout` is the wire and nothing else.** Any stray `println!` corrupts the
  stream. Chrome's own troubleshooting says "If you want to print some data
  for debugging purposes, write to `stderr`", and it redirects the host's
  stderr into the browser's error log. For a binary whose day job is printing
  to stdout — including under `--json` — this needs to be structural, not a
  convention. [documented]
- **Windows needs `O_BINARY`.** The default `O_TEXT` mode "corrupts the
  message format as line breaks (`\n` = `0A`) are replaced with Windows-style
  line endings (`\r\n` = `0D 0A`)". Rust's `std::io::stdout` does not do text
  translation, so this is a C-runtime hazard rather than a Rust one
  [inferred], but it belongs in a test on the Windows CI leg rather than in a
  belief.

### 1.7 The size caps — and which one actually binds

This is where the proposal as it reached this desk was wrong, and the
correction is good news.

| Direction | Cap | Enforcement |
|---|---|---|
| host → Chrome | **1 MiB** (`kMaximumNativeMessageSize = 1024 * 1024`) | Hard. Exceeding it logs at ERROR and calls `Close(kHostInputOutputError)`, tearing down the whole channel. The check runs as soon as the 4-byte header is available, *before* the body is buffered. [source] |
| extension → host | **64 MiB** (`mojom::kMaxMessageBytes`) | Enforced in the renderer, in `MessageFromV8()`: `if (message_size > mojom::kMaxMessageBytes) { *error = "Message exceeded maximum allowed size of 64MiB."; }`. The comment explains why: `// IPC messages will fail at > 128 MiB. Restrict extension messages to 64 MiB.` The `postMessage` call fails synchronously; the host never sees it. [source] |

`native_message_process_host.cc` contains a `kMaxSizeToRecord = 1024 * 1024 * 64`
commented "Set the max value to the 64MB message size limit", but it is a UMA
histogram bound only — nothing in that file rejects an oversized outbound
message, because the rejection already happened upstream. [source]

**Our payload travels in the 64 MiB direction.** Measured against synthetic
tabs with realistic URL and title lengths, full `chrome.tabs` field set,
compact JSON: [measured]

```
   300 tabs ->    143,878 bytes  (  0.137 MiB)  OK vs 64 MiB ext->host   OK vs 1 MiB host->Chrome
  1000 tabs ->    479,547 bytes  (  0.457 MiB)  OK vs 64 MiB ext->host   OK vs 1 MiB host->Chrome
  5000 tabs ->  2,397,822 bytes  (  2.287 MiB)  OK vs 64 MiB ext->host   OVER vs 1 MiB host->Chrome
 20000 tabs ->  9,605,167 bytes  (  9.160 MiB)  OK vs 64 MiB ext->host   OVER vs 1 MiB host->Chrome
```

300 tabs is 140 KB — 0.2% of the ceiling. The 64 MiB limit is reached at
roughly **140,000 tabs**. Extrapolating the same bytes-per-tab, the 1 MiB
host→Chrome limit is reached at about **2,200 tabs** — which is why the
reply must stay a status line and must never, ever carry archive contents
back. (It must not anyway; see §6.)

The 4.3 MB session files in the test corpus are **not** the relevant
comparison. An SNSS file is an append log containing every navigation of
every tab over the browser's whole run, plus superseded records. The live tab
list is one URL and one title per tab. The two are different quantities and
the big one does not go over this wire.

**Verdict: no chunking, no streaming, no sequence headers.** One message in,
one status out. If a future payload ever approached 64 MiB, the right answer
would be to reconsider what we are sending, not to build a framing protocol.

---

## 2. Who launches whom

### 2.1 Chrome is the parent, always

Chrome spawns the host. The host cannot spawn Chrome, cannot connect to a
running Chrome, and cannot ask Chrome for anything. On `connectNative` the
process lives for the port's lifetime; on `sendNativeMessage` it lives for one
request. On POSIX shutdown Chrome sends `SIGTERM` then `SIGKILL`; on Windows
the host is placed in a Job object that gets killed. [documented]

So the control flow of the product inverts. Today: *user or cron runs
`knowmoretabs save`, which reads a file.* With native messaging: *the browser
runs `knowmoretabs`, which is handed tabs it did not ask for.*

### 2.2 One binary, two entry points

A **hidden subcommand is not available to us.** The host manifest's `path` is
a path to an executable and nothing else — there is no `args` key. On macOS
and Linux it must be absolute. Chrome constructs the argv itself, and the
host receives exactly:

```
argv[0] = <the path from the manifest>
argv[1] = chrome-extension://<32-char id>/
argv[2] = --parent-window=0                 # Windows only, 0 from a service worker
```

[documented]

**Therefore: argv detection, and specifically `argv[1]` detection, before
clap ever sees the arguments.** In `main`, before `Cli::parse()`:

```rust
if let Some(origin) = std::env::args_os().nth(1) {
    if let Some(origin) = origin.to_str() {
        if origin.starts_with("chrome-extension://") {
            return native_host::run(origin);   // stdout is the wire from here on
        }
    }
}
```

This is not a heuristic hack — `argv[1]` is a documented part of the calling
convention, and we have to inspect it regardless, because validating the
origin against our pinned extension id is a security requirement (§6). The
dispatch and the check are the same read.

An `argv[0]`-based alternative (install a hardlink named
`knowmoretabs-native-host` and dispatch on the executable's own name) is also
viable and is arguably more legible in a `ps` listing, at the cost of a second
file on disk that the install step must create and the uninstall step must
remove. Prefer `argv[1]`: it is one file, it is the documented contract, and
a user who runs `knowmoretabs chrome-extension://…` by hand has explicitly
asked for host mode.

Three consequences for `cli.rs`:

- Clap never sees host mode. `after_help`, `--help` and the error text stay
  exactly as they are.
- Host mode must not accept `--root`, `--browser` or any other flag from the
  wire or from argv. It computes the archive root the same way `save` does,
  from defaults and environment only. This is a security property, not
  ergonomics (§6).
- Exit status 3 ("refused because encrypted files are newer") is meaningless
  in host mode — there is no session file involved. Host mode needs its own
  small status vocabulary in the JSON reply, not an exit code, because
  nobody is reading the exit code.

### 2.3 `save` from cron, while the browser holds an instance

**Cron cannot get live tabs at all.** This is the finding that most changes
the shape of the feature.

There is a mechanism in Chromium for a native application to initiate a
connection to an extension — `LaunchNativeMessageHostFromNativeApp` in
[`native_messaging_launch_from_native.cc`](https://chromium.googlesource.com/chromium/src/+/HEAD/chrome/browser/extensions/api/messaging/native_messaging_launch_from_native.cc).
It is not usable. `ExtensionSupportsConnectionFromNativeApp` requires, in
order: a non-incognito profile; the extension enabled; the host id listed in
the extension's **`natively_connectable`** manifest field; the
`nativeMessaging` permission; the **`transientBackground`** permission; and a
live `runtime.onConnectNative` listener. [source]

And `natively_connectable` is declared in
`chrome/common/extensions/api/_manifest_features.json` with
**`"channel": "dev"`**. [source] It does not exist on stable Chrome. The
policy path in that file is explicitly unfinished — there is a TODO noting
that policy still needs to be applied for `allow_user_level`.

So: on stable Chrome, on any Chromium-family browser a normal person uses,
**there is no supported way for a command-line process to pull tabs out of the
browser.** The browser must push.

That leaves exactly one design:

- The **extension owns the schedule.** `chrome.alarms` with a periodic alarm
  (minimum period 30 seconds since Chrome 120 [documented]), plus
  `chrome.runtime.onStartup` for a snapshot at browser launch, plus a toolbar
  action for "save now".
- **cron keeps doing what it does today**, reading session files. It is not
  replaced; it is unaffected. When encryption stage 3 lands it will start
  refusing, exactly as designed, and the extension will be the thing still
  producing snapshots.
- **When Chrome is not running, there is no live source.** A 3 a.m. cron job
  on a laptop whose browser is quit gets nothing from the extension. It
  currently gets a perfectly good snapshot from the session file. After stage
  3 it gets a refusal. That gap has no fix.

This also means the documented "run `save` from cron" workflow does not
survive the migration in its current form, and the honest thing to do is say
so in the README rather than imply the extension covers it.

### 2.4 Service worker lifetime

Worth knowing because it constrains the choice in §1.1. Extension service
workers shut down after **30 seconds of inactivity**; a single event handler
running more than 5 minutes is killed; a `fetch()` over 30 seconds is killed.
[documented]

Relevant exceptions, by version: [documented]

| Behaviour | Since |
|---|---|
| A native messaging host connected via `connectNative()` keeps the worker alive | Chrome 105 |
| Any extension API call resets the idle timer | Chrome 110 |
| Sending over a long-lived port keeps it alive; *opening* a port no longer resets timers | Chrome 114 |
| `chrome.alarms` minimum period of 30s, matching the lifecycle | Chrome 120 |

And the trap: "if the host process crashes or is shut down, the port is
closed and the service worker will terminate after timers complete."
[documented] A `connectNative` design therefore couples our process lifetime
to the worker's, and a host crash silently ends the worker. `sendNativeMessage`
has none of this: fire, get a reply, done, worker idles out normally. Use it.

### 2.5 The archive lock

`src/archive.rs` already has what is needed and one gap.

The existing `Archive::lock(on_wait)` uses `File::try_lock()` and, on
`TryLockError::WouldBlock`, calls `on_wait()` and then **blocks** on
`File::lock()` — `flock` on Unix, `LockFileEx` on Windows. The module header
already records the important asymmetry: `flock` is advisory, `LockFileEx` is
mandatory and also fails other processes' reads of the locked range.

Interaction with a browser-spawned instance, point by point:

1. **Both processes are `knowmoretabs`, so the advisory lock is sufficient.**
   `flock`'s advisory nature only matters against a process that never asks
   for the lock, and there is no such process here. Nothing changes.
2. **The host must take the lock around the write, not around the
   connection.** With `sendNativeMessage` this is automatic — the process
   exists for one message. With `connectNative` it would be a real bug: a
   host holding the archive lock for a browser session lasting hours would
   block every cron `save` on the machine. Another argument for
   `sendNativeMessage`.
3. **The host must not block.** A blocking `lock()` means Chrome's callback
   never fires and the service worker sits waiting. Host mode needs a
   `try_lock`-and-report path: on `WouldBlock`, reply
   `{"status":"busy"}` and exit 0, and let the extension's next alarm retry.
   That is a small addition to `archive.rs` — expose the try-only variant
   rather than only `lock(on_wait)` — and it is the one change to existing
   code this feature forces.
4. **Concurrency is already correct once (3) is done.** Two browsers, two
   profiles, and a cron job all writing into one archive root serialise on
   one lock. Same-second collisions already get the `-2`, `-3` suffix.
5. **The "unchanged session" skip needs a source dimension.** `save` today
   skips a snapshot whose `LayoutEntry` list matches the previous snapshot.
   A live-tabs snapshot and a session-file snapshot of the same browser will
   produce *nearly* identical layouts, so without scoping the comparison to
   the previous snapshot **of the same source kind**, the two sources will
   suppress each other's snapshots non-deterministically depending on which
   ran last. This is a genuine correctness issue and belongs in the first
   slice of the work, not a later one.

### 2.6 What the data model has to grow

`Source` today is file-shaped: `path`, `file`, `sha256`, `bytes`, `saved_at`,
`session_started_at`. A live capture has none of those. It has a browser, a
profile, an extension id, a browser version, and a capture timestamp.

The cheapest honest change is a tagged `kind` plus optionality:

```
Source { kind: "session-file" | "live-tabs", browser?, profile?, profile_display?,
         path?, file?, sha256?, bytes?, saved_at?, session_started_at?,
         extension_id?, browser_version? }
```

and a `schema_version` bump. Readers already "tolerate unknown fields" per
the brief, so old readers survive; new readers must handle absent `path`.

And one loss that should be recorded in the data model rather than discovered
later: **there is no verbatim artifact for a live capture.** Every session-file
snapshot directory contains `session.snss`, which means a better parser next
year can re-derive a better `snapshot.json` from a snapshot taken today. A
live-tabs snapshot directory contains the JSON Chrome handed us and nothing
else. Storing the raw `chrome.tabs` response verbatim alongside the normalised
`snapshot.json` is the closest equivalent and costs about 140 KB per snapshot.
Do it.

---

## 3. Data fidelity versus session files

### 3.1 What `chrome.tabs` gives that SNSS does not

| | Detail |
|---|---|
| **No write lag** | SNSS is what the browser has flushed. The API is what the browser has. A tab opened ten seconds ago is in one and may not be in the other. |
| **No format risk** | The single biggest recurring cost in this project is Chrome changing its session command IDs without notice — weaknesses 1, 2 and 3 in the brief all exist because of it. `chrome.tabs.Tab` is a documented, versioned, deprecation-policied interface. Fields are *added*, at named milestones. |
| **Tab groups, first-class** | `Tab.groupId` for membership (ungated, Chrome 88+) and `tabGroups.query()` for `title`, `color`, `collapsed`, `windowId`, `shared` (Chrome 137+). Compare with reverse-engineering commands 25 and 27 and hoping the token layout has not changed. |
| **Favicons** | `favIconUrl`. SNSS has no favicon at all. This is the single most visible improvement the library UI would get. |
| **Liveness signals** | `audible`, `mutedInfo` (Chrome 46+), `discarded` and `autoDiscardable` (Chrome 54+), `frozen` (Chrome 132+), `status` (`unloaded` / `loading` / `complete`). None of these exist in a session file. "Which of my 300 tabs are actually resident" is a question only the live API can answer. |
| **`lastAccessed`** | Chrome 121+, milliseconds since epoch. We already parse an equivalent out of SNSS, so this is parity, not gain — but it is parity obtained for free. |
| **Other free fields** | `openerTabId` (the tab that spawned this one — a genuinely new relation we do not have today), `incognito`, `highlighted`, `width`/`height`, `sessionId`, `splitViewId` (Chrome 140+). |
| **Window state** | `chrome.windows.getAll` adds `state` (minimized/maximized/fullscreen), `type`, `focused` and bounds. |
| **Nothing encrypted is touched** | The whole point. |

[documented] throughout.

### 3.2 What SNSS gives that `chrome.tabs` does not

This is the part that decides the recommendation, so it is specific.

**1. Tabs after a crash.**

> Editorial note, added after this document was written: the brief that
> commissioned it asserted that crash recovery is "the reason this project
> exists". That claim was the orchestrator's, inherited from an earlier
> analysis and never made by the project's owner. The technical content below
> stands on its own evidence; the ranking it was used to justify — that an
> extension could only ever be a second source — does not, and has been
> withdrawn. How much the owner values recovering a crashed session against
> seeing live tabs is their call, and is not recorded anywhere yet.

When Chrome dies, the extension dies with it. Its service worker is a Chrome
process. There is no final flush, no exit handler that survives a SIGKILL or
a kernel panic or a battery pull, and no state anywhere on disk that the
extension wrote — unless the extension had already pushed a snapshot before
the crash, in which case what we have is the snapshot from the last alarm,
not the tabs at the moment of death. [inferred, but not really arguable]

The session file *is* the crash-recovery mechanism. It is an append log that
Chrome flushes as it goes, precisely so that Chrome itself can restore from
it, and it is on disk before the crash rather than because of it. At best an
alarm-driven extension is stale by its alarm period. At worst — browser
crashed during startup, extension disabled by an update, user just installed
Chrome on a new machine and is restoring — it has nothing at all.

The whole pitch of the product is "one crash away from gone, and we are the
reason it isn't." A live-tabs-only knowmoretabs cannot make that claim.

**2. It works with the browser closed.**

`knowmoretabs save` today does not care whether Chrome is running. §2.3
establishes there is no stable-channel mechanism for the CLI to pull from the
browser, so after the migration there is simply no way to capture a browser
that is not running.

**3. Navigation history per tab.**

SNSS command 6 records navigation entries per tab, with the pruning semantics
of commands 5, 11 and 24 that the brief singles out as "subtle". That is the
back/forward list: where this tab has been, not just where it is now.

`chrome.tabs` gives exactly **one** URL per tab. No extension API exposes a
tab's back/forward list. `chrome.history` is a different dataset — per-profile
visits over time, with no tab attribution, missing incognito and
history-disabled visits — which is precisely the confusion this brief was
right to correct. The only API that would give per-tab navigation entries is
the debugger API's `Page.getNavigationHistory`, which attaches a
"knowmoretabs is debugging this browser" banner to every tab. That is not a
trade anyone would make.

We do not surface navigation history in the UI today. That is not a reason to
be relaxed about losing it — it is in the archive, it is recoverable later,
and once a snapshot is taken without it, it is gone forever.

**4. Closed windows and the previous session.**

`Session_*` for the previous run is a complete record of the last browser
session, including windows the user closed. `chrome.sessions.getRecentlyClosed()`
returns at most `MAX_SESSION_RESULTS = 25` entries of *recently* closed tabs
and windows [documented] — a small LRU, not an archive, and as established in
§1.2 it costs the worst prompt on the list.

**5. The verbatim artifact.** Covered in §2.6. Re-parsing an old snapshot with
a better parser is a capability that only exists for file-based sources.

### 3.3 The conclusion this forces

`chrome.tabs` is a **richer** description of a **narrower** moment: everything
Chrome currently has in memory, described in more detail than a session file
ever could, and nothing else. SNSS is a **poorer** description of a **wider**
window: less metadata per tab, but including the moments the browser was not
alive to describe.

They are not substitutes in either direction. That is the whole argument for
"second source", and it does not change when one of them stops working.

---

## 4. Does it retire the encrypted-sessions clock?

**No — it retires half of it. An extension keeps working when Chrome encrypts
session storage, so "what is open right now" is safe; "what was open when
Chrome died" has no answer that does not read a session file, and the
migration takes that answer away with nothing to replace it.**

Point by point:

**Does an extension keep working when Chrome encrypts session storage?** Yes.
The encryption lives entirely in `components/sessions`' storage layer — v5
files, `SerializeWithEncryption`, the OSCrypt provider chain — none of which
is on the path between `chrome.tabs.query()` and the browser's in-memory tab
model. The extension asks the browser a question and the browser answers from
memory. Chrome also still decrypts its own files for its own purposes, so
`chrome.sessions` keeps working too. [inferred, from the encrypted-sessions
report's source reading; the affirmative claim is not separately documented
because it is the absence of a coupling rather than the presence of one]

**Brave, Edge, Vivaldi, Chromium?** Yes, all four, with per-browser install
locations (§1.4) and, on Windows, per-browser registry hives. Edge documents
an explicit fallback to the Chromium and Chrome registry keys, which softens
the Windows story slightly and complicates it in one specific way (§1.4:
first key found wins, so one manifest may have to list two store ids).
Chrome for Testing has had its own paths since Chrome 146. [documented]

This also quietly answers the **`arc` future row**: Arc is Chromium-based, so
native messaging reaches it, and a live-tabs source is exactly the "second
source adapter, not a row in the browser table" that row asks for. Whether
Arc ships the extension plumbing unmodified is unverified. [inferred]

**Firefox?** The same architecture, reachable, but it is not the same work.
Differences that matter: the host manifest key is **`allowed_extensions`**
(add-on ids) rather than `allowed_origins` (`chrome-extension://` URLs); the
add-on must declare an explicit id via `browser_specific_settings`, which
Chrome does not support, so it needs its own extension manifest; the manifest
directories and the registry hive
(`HKCU\Software\Mozilla\NativeMessagingHosts\<name>`) are different; and the
argv contract differs — Firefox passes the manifest path and (55+) the add-on
id, Chrome passes the origin. The wire format is identical: 32-bit native-order
length, UTF-8 JSON, 1 MB app→browser. [documented]

But there is a catch that makes this a non-question today: **knowmoretabs does
not support Firefox at all.** Firefox does not write SNSS; it writes
`sessionstore-backups/recovery.jsonlz4`. Slice 4's browser table is
Chromium-only by construction. A Firefox extension would not be a second
source for an existing browser — it would be first-ever support for a new
browser, delivered by a route we do not use for any other browser. That is a
separate product decision, and a defensible one, but it is not this one.

**Safari?** **Different project, and it breaks a principle.** Safari has no
native messaging host manifest and no stdio host process. `sendNativeMessage`
routes to an **app extension inside a signed macOS app bundle** — an
`NSExtensionRequestHandling` principal class, typically
`SafariWebExtensionHandler`, with `NSExtensionPointIdentifier` set to
`com.apple.Safari.web-extension`. `connectNative` ports are not supported.
The toolchain is Xcode; the extension must ship inside an app; the app and
extension must be code-signed and either notarised for Developer ID
distribution or shipped through the Mac App Store. [documented]

That is incompatible with brief principle 6, "One binary, no runtime. The user
installs nothing else." Supporting Safari means shipping a Mac app. Do not.

---

## 5. Distribution

This is the hard part, and it is hard in two independent places: getting the
extension onto a browser, and getting a JSON file into a specific OS location.

### 5.1 Getting the extension installed — three paths, two dead

**Unpacked / developer mode.** Free, instant, no account. Costs:

- The user must open `chrome://extensions`, enable Developer mode, and click
  "Load unpacked" on a directory they downloaded. Every security instinct
  a careful user has says not to do this, and they are broadly right.
- **We cannot script it.** Chrome 137 removed the `--load-extension` flag in
  **branded Chrome builds**, explicitly because it "was commonly abused to
  load malicious and unwanted software into the browser". It still works in
  Chromium and Chrome for Testing, and in other Chromium browsers such as
  Brave. [documented, via the
  [Chromium Extensions PSA](https://groups.google.com/a/chromium.org/g/chromium-extensions/c/1-g8EFx2BBY/m/S0ET5wPjCAAJ)
  and the [June 2025 extensions blog](https://developer.chrome.com/blog/extension-news-june-2025)]
  So "our installer loads the extension for you" is not available on the
  browser most users have.
- Unpacked extensions are lower-trust and get auto-disabled on some Chrome
  updates, requiring a manual re-enable. The exact current behaviour and
  whether a startup bubble still appears could not be established from a
  primary source; the secondary sources found were low quality and are not
  worth citing. [inferred — needs checking on a real profile before it
  appears in a README]
- The extension id is derived from the directory path unless `key` is
  pinned, and `allowed_origins` cannot be written without a known id
  (§5.3). [documented]

**Self-hosted CRX.** Dead. Chrome's own distribution documentation is
unambiguous: external installs from a local CRX path were blocked on Windows
from Chrome 33 and on macOS from Chrome 44, and on both platforms
`update_url` "must point to the Chrome Web Store". Only Linux can install from
a local CRX or a personally-hosted update manifest. [documented] A
Linux-only distribution channel is not a distribution channel.

**Enterprise policy.** Dead for our users. `ExtensionInstallForcelist` with an
off-store update URL only takes effect on machines joined to Active
Directory / Entra ID, enrolled in Chrome Enterprise Core, or MDM-managed on
macOS; on an unmanaged personal machine the off-store entry is silently
ignored. [documented, via
[Set Chrome app and extension policies](https://support.google.com/chrome/a/answer/7532015)]
Worth documenting for the handful of users on managed work laptops. Not a
plan.

**Chrome Web Store.** The only path that works on a personal machine on
branded Chrome. Costs:

- A **one-time registration fee** and a Chrome Web Store developer account.
  The current registration page states the fee exists but not its amount; the
  long-standing figure is **$5 USD**, unchanged since 2010. [documented for
  "one-time fee"; [inferred] for the amount]
- The developer email address is **permanent** — "After an account is
  created, you cannot change its email address." For a project published under
  an organisation account this is a decision to make once, carefully.
  [documented]
- **Review.** "For most extensions, review is completed within a few days, but
  it can take up to a few weeks." Longer for "new developers", "new
  extensions", and — unhelpfully for us — "dangerous permission requests".
  The review documentation names `tabs` explicitly as a permission that
  "must be justified as necessary". **Every update is reviewed too**, and
  published items are re-reviewed periodically, with outcomes ranging from a
  warning with 7–30 days to fix, through takedown, to silent takedown and
  permanent account suspension for the serious cases. [documented]
- Unlisted visibility is available, so "published openly" does not have to
  mean "listed in a category alongside coupon extensions".

**So: what does a personal tool published openly realistically do?** It pays
the five dollars, publishes unlisted or public, writes one honest paragraph
in the listing explaining why it asks to "Read your browsing history", and
accepts that every release now has a review queue in front of it. Dev-mode
distribution is a viable *development* workflow and an unviable *product*
workflow, because it asks every user to do the one thing Chrome spends real
engineering effort telling them not to do — and then breaks on some future
Chrome update.

The uncomfortable second-order effect: a tool whose selling point is "no
accounts, no cloud, nothing leaves your machine" would acquire a dependency
on a Google developer account and a review process that can remove it from
users' browsers without notice. That is worth saying out loud in the README
rather than discovering.

### 5.2 Getting the host manifest installed — `knowmoretabs` grows an install step

Today the tool has no install step. You put a binary somewhere and run it. It
reads one browser file read-only, and writes only under `~/.knowmoretabs`.

Native messaging ends that. Something must write
`com.knowmoretabs.host.json` into each browser's `NativeMessagingHosts`
directory, or a registry key on Windows, containing the **absolute path of the
binary**. New surface:

- **`knowmoretabs install-host` and `uninstall-host`.** Both, symmetrically.
  A tool that scatters files into browser profile directories and cannot
  remove them is not the tool described in this brief.
- **The binary's path becomes load-bearing.** `path` is absolute (on
  macOS/Linux, by rule). Move or rename the binary — `brew upgrade`, a
  `cargo install` into a different prefix, moving `~/.local/bin` — and the
  host stops resolving, with a failure that surfaces as a browser-console
  message the user will never read. `install-host` needs a `--check` mode
  that verifies the recorded path still points at this binary, and `save`
  should probably run that check and warn.
- **macOS and Linux are nearly free.** `platform.rs::user_data_dirs()`
  already returns the right directory for all seven browsers on both
  platforms, tested. Add one path component and serialise five JSON keys.
- **Windows is the new work.** Registry writes mean either a crate
  (`windows-registry` / `winreg`) or hand-rolled FFI. `rust-stack.md`'s rule
  is that a dependency must remove a concrete correctness or portability
  risk; a registry crate clears that bar, but it is a new dependency in a
  project that currently has a deliberately small tree, and it can only be
  exercised on the Windows CI leg.
- **Per-browser install matrices are a known tarpit, not a theoretical one.**
  `bukubrow-host` ships `--install-brave`, `--install-vivaldi`,
  `--install-chromium`, `--install-edge` and `--install-dir` because one path
  does not fit; Bitwarden has an open issue about the manifest not being
  created for Brave; the Claude Code CLI has open issues about native
  messaging host detection failing specifically on Brave and Chromium. Every
  one of those is the same bug: seven browsers, three platforms, twenty-one
  cells, each of which must be right.
- **It puts a file inside the browser's own directory.** The brief's non-goals
  say "We never modify the browser's own files — we read and copy, nothing
  else." Writing a *new* file into the browser's user-data directory is not
  modifying a browser file, and the distinction is real — but it is close
  enough that the sentence needs rewording rather than lawyering. The brief
  also says, in the same list, "No browser extension (for now)." **Building
  this requires an explicit amendment to §1 of `BRIEF.md`, and that is a
  product decision, not an implementation detail.**

**What it costs us, in one sentence:** the tool stops being a binary you can
run and becomes a thing you install, which is a category change in what we
are asking of a user and cannot be undone by making the install step nice.

### 5.3 The `key` pin, which ties §5.1 and §5.2 together

`allowed_origins` needs the extension id at *manifest-write* time. An unpacked
extension's id depends on its directory path. So either:

- pin `key` in the extension manifest, giving a stable id everywhere,
  obtained by uploading a zip to the developer dashboard and copying the
  public key from the Package tab [documented]; or
- have `install-host` take the id as an argument, which means the user copies
  a 32-character string out of `chrome://extensions` into a terminal. That is
  a bad experience and a worse one to support.

Pin the key. Which means touching the developer dashboard — and therefore
paying the fee — even to make the dev-mode path work properly. The two
distribution questions are not independent.

---

## 6. The security story

### 6.1 What the binding actually guarantees

`allowed_origins` is enforced with a `URLPatternSet` restricted to
`URLPattern::SCHEME_EXTENSION`, and patterns where `match_all_urls()` or
`match_subdomains()` is true are rejected with "is not allowed."
[source] There is no syntax for "any extension". The binding is to exact
extension ids, and Chrome refuses to let you write it loosely. That is a
genuinely good design and it is the strongest part of this story.

### 6.2 What a malicious or compromised extension can do

**An unrelated malicious extension: nothing.** It cannot connect. Its id is
not in `allowed_origins`, and it cannot forge the origin — Chrome supplies it.

**Our own extension, compromised: whatever our wire protocol allows.** The
realistic route is a Web Store account takeover or a malicious update shipped
under our id, and in that scenario the attacker's JavaScript runs with our
extension id and speaks to our host with full legitimacy. Three design rules
follow, and they should be written into the host module's header as its
`why:`:

1. **The host takes no paths from the wire.** No `root`, no output path, no
   filename, no browser or profile override. It computes the archive root
   exactly as `save` does, from defaults and environment. Otherwise a
   compromised extension gets arbitrary file writes as the user.
2. **The host is write-only.** It accepts tabs and returns a status. It never
   returns snapshot contents, never lists the archive, never echoes a URL
   back. The archive is a record of everything the user has ever had open;
   a readback command turns a compromised extension into a
   browsing-history exfiltration channel with no network permission required.
   This is also why the 1 MiB host→Chrome cap is comfortable: nothing large
   should ever travel that way.
3. **The host executes nothing and spawns nothing.** No `export` trigger, no
   `serve`, no opening a browser, no shelling out.

A fixed vocabulary of one verb — `save` — satisfies all three, and
`sendNativeMessage` naturally enforces one message per process.

One more: the host must **validate `argv[1]` against the pinned extension id**
rather than merely checking it starts with `chrome-extension://`. Chrome
already checks `allowed_origins`, so this is belt-and-braces — but the manifest
on disk is user-writable (§6.3) and this check is not.

### 6.3 What a local process can do to the manifest

Any process running as the user can create or overwrite
`~/Library/Application Support/Google/Chrome/NativeMessagingHosts/com.knowmoretabs.host.json`,
point `path` at any executable, and list any extension id in
`allowed_origins`. There is no signature, no checksum, and no ownership
requirement on the user-level path. (Chrome *does* impose ownership
requirements on the external-extension preferences files — root-owned,
`admin`/`wheel`, not world-writable, no symlinks — explicitly so "an
unprivileged user" cannot install for all users [documented]. No equivalent is
documented for user-level native messaging manifests.)

Two honest observations that pull in opposite directions:

- **It is not a privilege escalation.** A process running as the user can
  already replace the `knowmoretabs` binary itself, edit the user's shell
  profile, or install a launch agent. The manifest adds no authority that was
  not already there.
- **It is a new kind of foothold.** It converts "can write files as the user"
  into "can be started by the browser, with the browser as parent, on the
  browser's schedule, reachable from extension JavaScript." That is a more
  useful primitive to an attacker than an ordinary file write, and it is one
  our install step teaches users to expect files to appear in.

The mitigation that exists is administrative: Chrome's
`NativeMessagingAllowlist` / `NativeMessagingBlocklist` policies (a generic
`ListPolicyHandler` subclass that validates each entry with
`NativeMessagingHostManifest::IsValidName` and accepts `"*"` when wildcards are
permitted [source]) and `NativeMessagingUserLevelHosts`, which turns off
user-level hosts entirely. Irrelevant on a personal machine; relevant on a
managed one, where it may mean the feature simply does not work.

### 6.4 The unsandboxed child

The host runs **as the user, unsandboxed, as a child of the browser**. It is
not subject to any of Chrome's renderer sandboxing. On macOS, a process
spawned by Chrome may be attributed to Chrome for TCC purposes, so file-access
prompts our host triggers could appear as *Chrome* asking for access —
confusing at best. [inferred; not verified, and not verifiable without
installing a host on this machine, which this brief forbids]

### 6.5 Compared with today, plainly

| | Today | With native messaging |
|---|---|---|
| Permission prompts | None | "Read your browsing history." + "Communicate with cooperating native applications." + "View and manage your tab groups." |
| Files written outside the archive | None | One JSON file per browser profile, or a registry key per browser |
| Browser files | Opened read-only, copied | Same, plus a new file in the user-data directory |
| Code launched by another program | None | Our binary, by the browser, on the browser's schedule |
| Attack surface reachable from a browser | None | A JSON parser reading an untrusted length prefix |
| Third-party account dependency | None | A Chrome Web Store developer account that can be suspended |
| Trust story in one line | "It reads one file and asks for nothing." | "It reads one file, and also installs a browser extension that asks to read your browsing history." |

That last row is the real cost. The encrypted-sessions report ended by saying
the tool's "strongest trust property is that it reads browser data without
asking for browser secrets." Native messaging does not ask for browser
*secrets* — that distinction holds, and it is the reason this is the right
fallback rather than decryption. But it does ask for a permission whose
user-facing string is "Read your browsing history", and no amount of
architectural correctness makes that prompt say something else.

---

## 7. Recommendation

**Build it as a second source. Do not make it primary, ever — not even after
sessions encrypt. Do not build it yet. Size it as three slices.**

### The argument

The four options collapse to two once §3.2 is taken seriously. "Make it
primary once sessions encrypt" is not a promotion we would be choosing; it is
a degradation we would be surviving, and calling it a promotion would let us
stop noticing what we lost. When stage 3 lands, live tabs become the *only
available* source on that browser — and a knowmoretabs that only sees live
tabs cannot recover a crash, cannot run with the browser closed, and cannot
record a tab's navigation history. That is a smaller product wearing the same
name, and the README would have to be rewritten to stop promising the thing
that made anyone install it. So: second source, permanently, with the
degradation named honestly in the docs rather than smoothed over.

"Do not build it and accept the clock" is also wrong, but only just. `save` is
the command. The brief's 80/20 says two things carry almost all the value and
`save` is the first. A tool whose primary command refuses to run is finished,
and "we detect the condition and print a good error" is a way of being
finished politely. Something has to exist on the other side of that error
message.

So build it — and the timing matters as much as the decision. Nothing in
`encrypted-sessions.md` establishes a date; the current Chromium default is
still stage 1, `write_both_read_only_clear`, and no authoritative source shows
stage 2 or 3 enabled by default on stable. Building a Chrome Web Store
listing, a registry installer and a twenty-one-cell install matrix *now*, to
answer a migration that might be four milestones away, is the opposite of
this project's stated approach to scope. The correct posture is: the detector
is already shipped in slice 1, this document is the plan, and the trigger is
either the detector firing on a real profile or a stage-2/3 default landing in
a stable milestone we can observe.

One thing is worth doing before then, and it is cheap: **scope the
"unchanged session" comparison to the previous snapshot of the same source
kind** (§2.5.5). Not doing it is a latent correctness bug that only appears
once a second source exists, and by then there will be snapshots in the wild.

### Size

**Three slices, and the third is the one that will hurt.**

| | What | Rough shape |
|---|---|---|
| **A. The host** | Framing (~35 lines of `std`, prototyped and working), `argv[1]` dispatch, origin validation, a `LiveTabs` source kind, the `Source` schema bump, the try-lock path in `archive.rs`, the same-source layout comparison, the MV3 extension (~150 lines of JS plus a manifest), `install-host`/`uninstall-host` for macOS and Linux, integration tests that drive the real binary over a real pipe. | The largest single piece, and the most self-contained. Every mechanism in it is documented and one of them is already prototyped. |
| **B. Windows and the rest of the table** | Registry read/write, a dependency decision, the per-browser install matrix across seven browsers and three platforms, `--check` for a moved binary, Windows `O_BINARY` verification, CI coverage. | Smaller in lines, larger in cells. This is where the bugs other projects have shipped will find us. |
| **C. Distribution** | Developer account, the fee, the pinned `key`, listing copy that justifies "Read your browsing history", a privacy disclosure, the review round-trip, update mechanics, and an amendment to `BRIEF.md` §1 removing "No browser extension (for now)". | **Mostly not code.** It has a queue in front of it that we do not control, and it recurs on every release. |

If someone insists on one number: assume A and B are each a normal slice, and
C is a normal slice's worth of calendar time containing a fortnight's worth of
waiting.

---

## Appendix: what could not be verified

- **Brave and Vivaldi native messaging paths** are [inferred] from the
  `DIR_USER_DATA + "NativeMessagingHosts"` rule in `chrome_paths.cc` and from
  third-party installers. Neither vendor publishes a native messaging page.
  Confirm on a real profile before slice code depends on them.
- **Windows registry hives for Brave and Vivaldi** — same status, and no
  Windows machine was available here.
- **Whether Chrome still shows a startup warning for developer-mode
  extensions, and under exactly which conditions an unpacked extension is
  auto-disabled after an update.** No primary source found; the secondary
  sources were poor. This matters for §5.1 and should be checked on a real
  profile before it is asserted in a README.
- **The Chrome Web Store fee amount.** The registration page documents a
  one-time fee without a figure. $5 is well-attested but secondary.
- **`chrome.sessions` behaviour across a crash or a browser restart.** The API
  reference does not address it. It does not change the recommendation —
  we are not requesting the permission — but the claim "it does not survive a
  crash" is [inferred], not documented.
- **Firefox tab-group WebExtension API support.** Firefox shipped tab groups
  in 2025; whether a `browser.tabGroups` equivalent exists and at which
  version was not established. Immaterial unless Firefox support is ever
  considered.
- **macOS TCC attribution for a browser-spawned child process** (§6.4).
  Plausible, unverified, and unverifiable without installing a host — which
  this brief forbids, correctly.
