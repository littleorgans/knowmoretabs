# Encrypted Chrome session storage: decision report

## Executive answer

The three headline claims are all **yes**, with one important qualification:

1. **Feature/default/stage: yes.** Current Chromium `HEAD` defines
   `kEncryptSessionStorage` as `FEATURE_ENABLED_BY_DEFAULT`, with the default
   stage `write_both_read_only_clear`.
2. **This machine: yes.** The Chrome profile has both `Sessions/` and
   `Sessions_Encrypted/`; the files in the former have the v3 header and the
   files in the latter have the v5 header.
3. **Stage-3 behavior: yes.** The source stops writing cleartext in
   `write_encrypted_read_prefer_encrypted` and best-effort deletes the old
   cleartext session files.

The qualification is about history: M148 introduced the separate encrypted
directory, but the source history does not establish that full v5 encryption
and dual-write shipped in M148. The full implementation landed on Chromium
main on 2026-06-11, between M149 and M150. There is no trustworthy public
milestone or date for the stage-3 stable rollout. We should treat the change as
possible on any Chrome update and ship detection now.

**Verdict:** choose (b)—do not decrypt Chrome's encrypted session files; detect
the condition and refuse to create a snapshot that would silently be stale.

**Exact user-facing message:**

> Chrome's encrypted session files are newer than its cleartext session files. knowmoretabs cannot read the encrypted files, so this capture would be stale and no snapshot was saved. Update knowmoretabs when encrypted-session support is available, or use the live-tabs/DevTools fallback.

## What was independently checked

The live-profile check was deliberately limited to directory listings, `stat`,
and the first eight bytes of each listed session file. No file body was parsed,
no file was copied or changed, and no Keychain or other credential store was
accessed. No personal filenames, URLs, timestamps, or other profile data are
included here.

On this machine, both directories exist. All listed files under `Sessions/`
begin with:

```text
53 4e 53 53 03 00 00 00
```

All listed files under `Sessions_Encrypted/` begin with:

```text
53 4e 53 53 05 00 00 00
```

That independently confirms the claimed v3/v5 coexistence. It does not prove
that every Chrome profile or every Chrome channel is currently in the same
variation cohort; it proves the state of this profile.

The relevant Chromium source is clear:

- [`components/sessions/core/command_storage_features.cc:BASE_FEATURE(kEncryptSessionStorage)`](https://chromium.googlesource.com/chromium/src/+/HEAD/components/sessions/core/command_storage_features.cc)
  sets the feature to `base::FEATURE_ENABLED_BY_DEFAULT`, and its feature
  parameter defaults to `write_both_read_only_clear`.
- [`components/sessions/core/command_storage_features.h:EncryptSessionStorageStage`](https://chromium.googlesource.com/chromium/src/+/HEAD/components/sessions/core/command_storage_features.h)
  defines the four stages. The final stage is
  `write_encrypted_read_prefer_encrypted`.
- [`components/sessions/core/command_storage_manager.cc:ShouldWriteCleartextFiles`](https://chromium.googlesource.com/chromium/src/+/HEAD/components/sessions/core/command_storage_manager.cc)
  returns false in the final stage, so new cleartext files stop being written.
- [`components/sessions/core/command_storage_manager.cc:OnEncryptedBackendReadFinishedWithPreferEncrypted`](https://chromium.googlesource.com/chromium/src/+/HEAD/components/sessions/core/command_storage_manager.cc)
  invokes clear-backend cleanup in the final stage. The cleanup reaches
  [`components/sessions/core/command_storage_backend.cc:DeleteLastSessionFiles`](https://chromium.googlesource.com/chromium/src/+/HEAD/components/sessions/core/command_storage_backend.cc),
  which removes the old cleartext session files on a best-effort basis.

So the worst failure mode in the brief is real: a consumer that keeps reading
only `Sessions/` can continue to return old data without an I/O error.

## Timeline and what is actually known about stable

Chromium's release-cycle documentation points to the release calendar; the
Chrome release-notes pages are the stable-milestone record:
[`release_cycle.md`](https://chromium.googlesource.com/chromium/src/+/HEAD/docs/process/release_cycle.md),
[`Chrome 148`](https://developer.chrome.com/release-notes/148),
[`Chrome 150`](https://developer.chrome.com/release-notes/150), and
[`Chrome 153`](https://developer.chrome.com/release-notes/153).
For the relevant interval, the stable dates were M148 (2026-05-05), M149
(2026-06-02), M150 (2026-06-30), and M153 (2026-09-08); those dates are the
calendar context for the commit-to-milestone comparisons below, not claims
that a staged feature was enabled for every stable user.

| Stage or change | Evidence | Stable conclusion |
|---|---|---|
| Separate encrypted directory | Commit [`4c4c8e9`](https://chromium.googlesource.com/chromium/src/+/4c4c8e9), “Store encrypted session files in a different folder,” 2026-03-23; the current [`session_constants.h:kEncryptedSessionsDirectory`](https://chromium.googlesource.com/chromium/src/+/HEAD/components/sessions/core/session_constants.h) comment says the directory was added in Chrome 148. | M148 is supported for the directory/path change. It is **not** evidence that M148 had working v5 dual-write encryption. |
| Encryptor and stage scaffolding | Commit [`ec2ae3b`](https://chromium.googlesource.com/chromium/src/+/ec2ae3bd652272d365ae2ce410562407bf1ea519), 2026-05-13, fetched the encryptor and added the staged feature path. | This was after the M149 stable milestone, so it cannot establish an M148 or M149 stable implementation. |
| Stage 1 / v5 implementation | Commit [`d351161`](https://chromium.googlesource.com/chromium/src/+/d3511611e0e68f403836606ca62543cbfcd37bbe), 2026-06-11, added the dual-backend implementation for `kWriteBothReadOnlyClear`, including v5 serialization and staged behavior. | M150 (2026-06-30) is the earliest stable milestone that could contain the full implementation. This machine's M153 profile confirms that the feature is exposed in a current stable installation. |
| Stage 2 and stage 3 | The same staged implementation and browser tests define the paths; for example, [`77737c3`](https://chromium.googlesource.com/chromium/src/+/77737c3ae3958142ec4768a3326faadb2524b732) added encrypted-session browser coverage. | I found no authoritative Chromium release note, launch milestone, or source-controlled rollout date showing stage 2 or stage 3 enabled by default on stable. The current source default remains stage 1. |

The answer to “when does cleartext stop on stable?” is therefore **unknown,
not a date we can responsibly predict**. A flag enum, a browser test, or a
feature being enabled by default at stage 1 is not evidence that a server-side
variation will advance to stage 3 in M156, M157, or any other milestone. The
product should not wait for a published date: the detection guard belongs in
slice 1.

## What v5 changes

The v3 and v5 file headers are readable framing. They are not an indication
that the entire file is encrypted.

[`components/sessions/core/command_storage_backend.cc:kFileVersionWithMarker`](https://chromium.googlesource.com/chromium/src/+/HEAD/components/sessions/core/command_storage_backend.cc)
defines v3 and `kFileVersionEncryptedWithOSCrypt` defines v5. The backend still
reads the SNSS header and command framing. The important difference is in
[`components/sessions/core/session_command.cc:SerializeWithEncryption`](https://chromium.googlesource.com/chromium/src/+/HEAD/components/sessions/core/session_command.cc):

- v3 stores a clear command size, command ID, and command payload.
- v5 stores a clear command size followed by an encrypted command record. The
  command ID is inside the encrypted record, along with the serialized
  payload. This applies to session commands generally, including marker and
  navigation/tab commands; it is not just a selected command type.
- The eight-byte SNSS header remains readable. The per-command length remains
  readable. The OSCrypt provider prefix (`v10`, `v11`, or `v20`) is also at the
  beginning of the encrypted blob, so a reader can identify the provider
  family without decrypting the command.
- For GCM-based encryption, the random nonce and authentication tag are part
  of the encrypted blob's framing. They are not secret, but they do not expose
  the command ID or payload.

Thus a v5-aware reader can enumerate record boundaries without possessing the
key, but it cannot reconstruct tabs, URLs, or commands from those records.

## OSCrypt and the key problem

Session encryption does **not** select one universal “session key format.” It
receives an `os_crypt_async::Encryptor`; Chrome chooses the highest-precedence
provider that can supply a key for encryption, while retaining provider keys
needed to decrypt older data. See
[`components/os_crypt/async/browser/os_crypt_async.h:OSCryptAsync`](https://chromium.googlesource.com/chromium/src/+/HEAD/components/os_crypt/async/browser/os_crypt_async.h)
and
[`components/os_crypt/async/common/encryptor.cc:EncryptString`](https://chromium.googlesource.com/chromium/src/+/HEAD/components/os_crypt/async/common/encryptor.cc).

| OS | Where Chrome protects the key | Session blob variant |
|---|---|---|
| macOS | The Chrome Safe Storage secret is in the macOS Keychain. Chromium's [`keychain_password_mac.mm:KeychainPassword`](https://chromium.googlesource.com/chromium/src/+/HEAD/components/os_crypt/common/keychain_password_mac.mm) uses the Chrome service/account entries; it derives the encryption key rather than storing the AES key as a readable file. | Legacy `v10`, AES-128-CBC. This is the legacy OSCrypt path, not app-bound `v20`. A third-party reader would need Keychain access and must handle access-control prompts and account/profile differences. |
| Linux | Usually the desktop secret store: GNOME Secret Service/libsecret or KWallet through the Freedesktop provider. If no usable store is available, Chromium's POSIX fallback uses a fixed local fallback secret; it is compatibility behavior, not a secure user credential store. | Secret-store provider is `v11`; the POSIX fallback is `v10`. Both are legacy OSCrypt families. See [`freedesktop_secret_key_provider.cc:FreedesktopSecretKeyProvider`](https://chromium.googlesource.com/chromium/src/+/HEAD/components/os_crypt/async/browser/freedesktop_secret_key_provider.cc) and [`posix_key_provider.cc:PosixKeyProvider`](https://chromium.googlesource.com/chromium/src/+/HEAD/components/os_crypt/async/browser/posix_key_provider.cc). |
| Windows | The legacy key is stored encrypted in Chrome's Local State and protected with Windows DPAPI. Current Chrome can instead protect the encryption key through the App-Bound Encryption elevation service. | Legacy DPAPI provider is `v10`; the current app-bound provider emits `v20` and uses AES-256-GCM. See [`dpapi_key_provider.cc:DPAPIKeyProvider`](https://chromium.googlesource.com/chromium/src/+/HEAD/components/os_crypt/async/browser/dpapi_key_provider.cc), [`app_bound_encryption_provider_win.cc:AppBoundEncryptionProviderWin`](https://chromium.googlesource.com/chromium/src/+/HEAD/chrome/browser/os_crypt/app_bound_encryption_provider_win.cc), and the provider ordering in [`browser_process_impl.cc:CreateOSCryptAsync`](https://chromium.googlesource.com/chromium/src/+/HEAD/chrome/browser/browser_process_impl.cc). |

The Windows answer is the critical one: **v20 is possible and is preferred
when App-Bound Encryption is supported.** A third-party binary cannot simply
read a v20 key from Chrome's profile. It would need to participate in Chrome's
elevation-service protocol or defeat that protection. That is a hard stop for
a reliable cross-platform implementation.

Even on macOS and Linux, decryption would turn a read-only session utility into
an application that requests access to browser credentials. That is a material
change to the “no scary permissions” promise.

## Feasibility verdict

Do not implement decryption in the KISS tool. Detection and a safe refusal are
the right product behavior.

A narrowly scoped macOS/Linux prototype is technically possible: it would need
per-command v5 parsing plus AES-CBC/GCM and PBKDF2 primitives, and native
Keychain/Secret Service/KWallet access. In Rust that likely means crypto crates
such as `aes`, `cbc`, `aes-gcm`, `hmac`, `sha1`, and `pbkdf2`, together with
platform bindings such as `keyring`/Security-framework and DBus bindings. The
portable logic is perhaps a few hundred lines; the platform, provider-selection,
credential-prompt, migration, and fixture tests are more realistically
hundreds to low thousands of lines. Windows app-bound support is not “add one
crate”: it requires Chrome's elevation-service contract and still would not be
portable to arbitrary third-party binaries.

That cost buys a credential-store integration, OS-specific failure modes, and
only partial coverage unless Windows v20 is solved. It is not justified for a
small archive tool whose strongest trust property is that it reads browser data
without asking for browser secrets.

## Detection logic to ship in slice 1

Run this preflight before reading or copying a session file:

1. Resolve the Chrome profile directory using the existing profile-discovery
   rules. `stat` the immediate `Sessions/` and `Sessions_Encrypted/`
   directories. If `Sessions_Encrypted/` does not exist, preserve current
   behavior and do not warn.
2. In each directory, enumerate immediate regular files only. Consider the
   `Session_`, `Tabs_`, and `Apps_` prefixes; ignore unrelated files. `stat`
   each candidate and record only its modification time. Do not open these
   files for this check.
3. For each prefix, compute the newest clear mtime and newest encrypted mtime.
   Also retain the directory mtimes as an existence/change diagnostic, but do
   not use directory mtime alone: appending to an existing file need not update
   its parent directory mtime.
4. Mark the profile **stale/unsupported** if any of these is true:
   - `Sessions/` is absent or has no candidate files while
     `Sessions_Encrypted/` has candidate files;
   - an encrypted prefix has no corresponding clear prefix; or
   - `encrypted_latest - clear_latest >= 300 seconds` for any prefix.
5. The five-minute threshold is intentional. Chrome can write the two
   backends a few seconds apart, and filesystems can have coarse mtime
   resolution; a five-minute gap filters that normal skew while catching the
   multi-minute/day-scale gap caused by a stage-3 migration. If encrypted data
   is newer but the gap is below five minutes, continue and check again on the
   next capture.
6. On stale/unsupported, stop before parsing or copying and show exactly the
   message quoted at the top of this report. Do not silently save the cleartext
   view. The safety check should not be bypassed by a normal “force” option,
   because that would recreate the invalid archive.

This logic is intentionally useful before encrypted-session support exists.
It reads metadata only, is safe when Chrome is running, and detects both the
dual-write lag and the post-migration state where cleartext files disappear.

## Ranked fallbacks

**1. Browser extension as a live source.** A small extension using
`chrome.tabs.query` is the best long-term fallback for current tabs: it avoids
profile files and OS credential stores, and can report URL/title/window/tab
metadata directly. The honest downside is installation and extension
permissions, plus loss of crash recovery, closed tabs, offline snapshots, and
possibly incognito data unless the user explicitly enables it.

**2. `Current Session`.** It is the cheapest file-based experiment because it
is close to the existing parser's domain and may contain the most recent live
state. It is not a dependable fallback: it can be absent, concurrently written,
truncated, or subject to the same encryption migration. It is also a current
session view, not a durable archive of every window/tab state.

**3. Chrome DevTools Protocol.** If the user has a Chrome instance exposing a
debugging endpoint, `Target.getTargets` can provide live page targets without
reading the profile. This is useful but operationally awkward: Chrome must be
started or configured for remote debugging, the endpoint is security-sensitive,
and the result is live tabs only. It does not recover a crashed or previously
closed browser.

**4. History DB.** A read-only copy/query of Chrome's History database can
provide visited URLs, titles, and visit times as a partial “what did I open?”
source. It cannot reconstruct current windows, tab ordering, groups, pinned
state, session boundaries, or reliable open-tab status; it also misses
incognito/history-disabled visits and has SQLite locking/copy-consistency
concerns. It is a different product datum, not a session-file substitute.
(The copy-consistency concerns are settled in
`docs/briefs/slice-07b-history.md` §3, which is how `save` reads History for
its per-tab signals.)

## Bottom line

The report's core risk is confirmed. v5 leaves enough framing to identify the
format but protects the useful command data, and Windows App-Bound `v20` makes
cross-platform decryption an especially poor foundation. The product should
remain a no-credential, read-only tool, detect the encrypted/cleartext mtime
divergence, and refuse to present stale session data as a fresh snapshot.
