# The SNSS session file format

> Reference for writing a robust Chrome/Chromium session parser without reading
> Chromium source again.
>
> slice: capture
> why: The parser is the one component that faces a moving target we do not
>      control. Everything we know about that target lives here, so a future
>      change can be diagnosed by diffing reality against this file rather than
>      by re-deriving the format.

Every claim below is tagged:

- **[chromium-source]** — read from Chromium `main` on 2026-09-20, file + symbol cited.
- **[observed-in-corpus]** — measured across the 8 real session files in
  `~/Documents/chrome-tabs/*/session-backup/` (6 × `Session_*`, 2 × `Tabs_*`,
  741 KB – 4.3 MB). Aggregate counts only; no URLs, titles or other content from
  those files appears anywhere in this repo.
- **[inferred]** — reasoned from the two above, not directly stated by either.

Chromium source paths are relative to `src/`. Where a file has moved, the current
path is given (`components/sessions/core/session_backend.cc` no longer exists —
it became `command_storage_backend.cc`; **[chromium-source]**, confirmed 404 on
`main`).

---

## 1. The container

### 1.1 File header

Eight bytes, then commands to EOF. **[chromium-source]**
`components/sessions/core/command_storage_backend.cc`, `struct FileHeader`,
`kFileSignature`, `SessionFileReader::ReadHeader`,
`CommandStorageBackend::OpenAndWriteHeader`.

```
offset  0    1    2    3    4    5    6    7
       +----+----+----+----+----+----+----+----+
       | 53 | 4e | 53 | 53 |  version int32 LE |
       +----+----+----+----+----+----+----+----+
         'S'  'N'  'S'  'S'
```

`kFileSignature` is the `int32_t` `0x53534E53`. Chromium's comment calls it
"SSNS (Sessions)", but because it is written as a little-endian `int32_t` the
bytes on disk read `SNSS`. Match the four bytes, not the comment.
**[chromium-source]** + **[observed-in-corpus]** (all 8 files start
`53 4e 53 53`).

| version | meaning | support |
|---|---|---|
| 1 | original cleartext | removed from Chromium 2021-05-25 (commit 223e5cd) |
| 2 | encrypted, never shipped | removed 2021-05-25 |
| 3 | `kFileVersionWithMarker`, cleartext | **current cleartext format** |
| 4 | `kEncryptedFileVersionWithMarker`, never used in production ("possible from early 2021 through early 2026") | not readable |
| 5 | `kFileVersionEncryptedWithOSCrypt` | **current encrypted format**, live in stable |

**[chromium-source]** version-number comment block at the top of
`command_storage_backend.cc` (`kFileVersionWithMarker`,
`kFileVersionEncryptedWithOSCrypt`, `// NEXT_VERSION = 6`).

Chromium itself accepts only versions 3 and 5
(`SessionFileReader::IsHeaderValid`), and 5 only when an `Encryptor` is
available, otherwise `ReadStatus::kUnsupportedVersion`. **[chromium-source]**

All 8 corpus files are version 3. **[observed-in-corpus]**

### 1.2 Command framing (version 3, cleartext)

Commands are appended back-to-back with no padding between them.
**[chromium-source]** `components/sessions/core/session_command.cc`,
`SessionCommand::SerializeAsCleartext`, `SessionCommand::GetSerializedSize`,
`SessionCommand::DeserializeCleartext`.

```
+---------+---------+---------+- - - - - - - - - - - - - - -+
| size    (uint16 LE) | id (u8) |  contents (size - 1 bytes)  |
+---------+---------+---------+- - - - - - - - - - - - - - -+
 <--- 2 bytes ------> <-1 byte-> <------ size - 1 bytes ----->

 bytes consumed from the file = 2 + size
```

Rules, all from `SerializeAsCleartext`/`DeserializeCleartext`
**[chromium-source]**:

- `size` counts the id byte **plus** the contents. `size >= 1` always; `size ==
  1` means a zero-length contents (the marker command, §2.3, is exactly this).
- `size` is a `uint16_t`, so `contents` is at most
  `kMaxContentSize = 65535 - 1 = 65534` bytes and a whole command occupies at
  most `2 + 65535 = 65537` bytes on disk. Contents longer than that are
  **silently truncated by Chrome at write time** — a pathological navigation
  entry can therefore be written malformed, not just large.
  (`session_command.h`, `kMaxContentSize`; `SerializeAsCleartext` logs and
  truncates.)
- Byte order is *native*, which on every platform Chrome ships for desktop is
  little-endian. A parser should hardcode little-endian and refuse anything
  that does not make sense. **[inferred]**

Largest command actually seen in the corpus: ~5.9 KB (a single command that
occupies the last 5 923 bytes of one file). `kCommandSetWindowWorkspace2`
payloads of ~2 KB are routine on macOS. Do not assume commands are small.
**[observed-in-corpus]**

### 1.3 Command framing (version 5, encrypted)

**[chromium-source]** `SessionCommand::SerializeWithEncryption`,
`DeserializeEncrypted`, `GetSerializedSize(data, encrypted=true)`.

```
+-------------------------+- - - - - - - - - - - - - - - - - -+
| length  (uint32 LE)     |  os_crypt_async ciphertext        |
+-------------------------+- - - - - - - - - - - - - - - - - -+
 <----- 4 bytes ---------> <-------- length bytes ----------->

 plaintext, once decrypted = [ id (u8) ][ contents ]
```

The size prefix widens from 2 bytes to 4 because ciphertext can exceed
`kMaxContentSize`. The id moves *inside* the encrypted blob, so an encrypted
file yields no command-id information at all without the key. See §7.

### 1.4 Torn tails, and how to tell them from corruption

These files are append logs written by a live browser. Reading one while Chrome
is running can catch a half-written command. Chromium's own reader treats this
as normal:

> "SessionFileReader does minimal error checking on the file (pretty much only
> that the header is valid)."
> — `command_storage_backend.cc`, comment above `class SessionFileReader`
> **[chromium-source]**

> "If the file is corrupt (command with wrong size, or unknown command), we
> still return true and attempt to restore what we we can."
> — `session_service_commands.cc`, comment in `CreateTabsAndWindows`
> **[chromium-source]**

`SessionFileReader::ReadCommand` distinguishes exactly three end conditions
**[chromium-source]**:

| condition | Chromium status | meaning |
|---|---|---|
| buffer exhausted with 0 bytes left over | `kSuccess` | clean end of file |
| fewer than 2 bytes left (cannot read `size`) | `kInvalidCommand`, logged `"file incomplete"` | torn tail |
| `size` read but fewer than `size` bytes follow | `kInvalidCommand`, logged `"last chunk lost"` | torn tail |

Note what is **not** on that list: there is no checksum, no length-of-file
field, no trailer. The only structural invariant is that `size` and the file
length agree. Therefore:

**The decision rule.** Walk commands from offset 8. At each step:

1. `remaining < 2` → **torn tail**. Report `remaining` bytes unparsed, keep
   everything already parsed.
2. `size == 0` → **torn tail** (Chromium rejects `size_field_value <
   sizeof(id_type)`). Same treatment. A run of zero bytes at the end of a file
   — a real possibility with a sparse or preallocated write — lands here.
3. `2 + size > remaining` → **torn tail**. Same treatment.
4. Otherwise the command is whole; consume it and continue.

A file is **corrupt** — as opposed to torn — only if the header is wrong: fewer
than 8 bytes, a bad signature, or an unsupported version. Everything past the
header degrades into "we recovered N commands and M trailing bytes". This is
the whole of the distinction. There is no third state. **[inferred from
chromium-source]**

Torn-tail behaviour measured by truncating real files **[observed-in-corpus]**:

```
Session_13433056410808181 (2 855 commands whole)
  -1 byte    -> 2 854 commands,   18 bytes unparsed, 1 command lost
  -17 bytes  -> 2 854 commands,    2 bytes unparsed, 1 command lost
  -500 bytes -> 2 829 commands,   10 bytes unparsed, 26 commands lost
  -5000 bytes-> 2 719 commands,    0 bytes unparsed (clean), 136 lost
```

Two things to take from this. First, a torn tail costs you the *last few*
commands, which are the most recent navigations — never the bulk of the
session. Second, truncation can land exactly on a command boundary and look
perfectly clean, so "0 bytes unparsed" is not proof the file is complete.
**[inferred]**

None of the 8 corpus files is torn: all parse to exactly 0 trailing bytes.
**[observed-in-corpus]** That is a property of how they were captured (the
reference implementation copies only a file whose `stat` is stable across the
read), not evidence that tearing is rare in general.

---

## 2. The command ID table

**Two different, overlapping, incompatible ID tables exist.** Which one applies
is determined entirely by the file name prefix. This is the single easiest way
to produce convincing garbage.

### 2.1 `Session_*` / `Apps_*` command IDs

**[chromium-source]** `components/sessions/core/session_service_commands.cc`,
lines 60–114 (`kCommandSetTabWindow` … `kCommandSetSplitTabData`) and the
`switch` in `CreateTabsAndWindows`.

Payload layouts marked **struct** are a raw `memcpy` of a C struct
(`CreateSessionCommandForPayload` → `command->contents().copy_from(
base::byte_span_from_ref(payload))`), so **C padding is on the wire**. Layouts
marked **pickle** use `base::Pickle` — see §3.

| ID | Name | Payload | Meaning | In the reference? |
|---|---|---|---|---|
| 0 | `SetTabWindow` | struct, 8 B: `i32 window_id`, `i32 tab_id` | tab belongs to window | handled ✅ |
| 1 | *SetWindowBounds* (obsolete) | — | superseded by 14 | **fatal** ❌ |
| 2 | `SetTabIndexInWindow` | struct, 8 B: `i32 tab_id`, `i32 index` | visual position in window; gaps allowed | handled ✅ |
| 5 | `TabNavigationPathPrunedFromBack` | struct, 8 B: `i32 tab_id`, `i32 index` | drop navigations with index ≥ `index` | handled ✅ (§6) |
| 6 | `UpdateTabNavigation` | pickle | one navigation entry (URL, title, …) | handled ✅ |
| 7 | `SetSelectedNavigationIndex` | struct, 8 B: `i32 tab_id`, `i32 index` | current entry in the back/forward list | handled ✅ |
| 8 | `SetSelectedTabInIndex` | struct, 8 B: `i32 window_id`, `i32 index` | active tab of a window, by *visual* index | allowlisted, ignored ✅ |
| 9 | `SetWindowType` | struct, 8 B: `i32 window_id`, `i32 type` | window becomes real (§2.4) | handled ✅ |
| 10 | *SetWindowBounds2* (obsolete) | struct, 24 B | still **read** by Chromium for migration | **fatal** ❌ |
| 11 | `TabNavigationPathPrunedFromFront` | struct, 8 B: `i32 tab_id`, `i32 count` | drop first `count` navigations | handled ✅ (§6) |
| 12 | `SetPinnedState` | struct, 8 B: `i32 tab_id`, `bool` + 3 B pad | pinned | handled ✅ |
| 13 | `SetExtensionAppID` | pickle: `i32 tab_id`, `string` | app/extension id | allowlisted ✅ |
| 14 | `SetWindowBounds3` | struct, 24 B: `i32 window_id, x, y, w, h, show_state` | geometry | allowlisted ✅ |
| 15 | `SetWindowAppName` | pickle: `i32 window_id`, `string` | **PWA / app window name** | **fatal** ❌ ⚠️ |
| 16 | `TabClosed` | struct, 16 B: `i32 tab_id`, 4 B pad, `i64 close_time` | forget this tab entirely | handled ✅ |
| 17 | `WindowClosed` | struct, 16 B: `i32 window_id`, 4 B pad, `i64 close_time` | forget this window (§2.4) | handled ✅ |
| 18 | *SetTabUserAgentOverride* (obsolete) | pickle | superseded by 29, still read | **fatal** ❌ |
| 19 | `SessionStorageAssociated` | pickle: `i32 tab_id`, `string` | sessionStorage namespace id | allowlisted ✅ |
| 20 | `SetActiveWindow` | struct, 4 B: `i32 window_id` | frontmost window | allowlisted ✅ |
| 21 | `LastActiveTime` | struct, 16 B: `i32 tab_id`, 4 B pad, `i64 µs since 1601` | tab last activated | allowlisted ✅ |
| 22 | *SetWindowWorkspace* (obsolete) | — | superseded by 23 | **fatal** ❌ |
| 23 | `SetWindowWorkspace2` | pickle: `i32 window_id`, `string` | desktop/space identifier | allowlisted ✅ |
| 24 | `TabNavigationPathPruned` | struct, 12 B: `i32 tab_id`, `i32 index`, `i32 count` | drop `count` from `index` | handled ✅ (§6) |
| 25 | `SetTabGroup` | struct, 32 B (§4) | tab's group membership | allowlisted, **ignored** ⚠️ |
| 26 | *SetTabGroupMetadata* (obsolete) | pickle | superseded by 27 | **fatal** ❌ |
| 27 | `SetTabGroupMetadata2` | pickle (§4) | **group title + colour** | allowlisted, **ignored** ⚠️ |
| 28 | `SetTabGuid` | pickle: `i32 tab_id`, `string` (UUID) | stable tab GUID | allowlisted ✅ |
| 29 | `SetTabUserAgentOverride2` | pickle: `i32 tab_id`, `string`, `bool`, `string?` | UA override | allowlisted ✅ |
| 30 | `SetTabData` | pickle: `i32 tab_id`, `i32 n`, `n × (string,string)` | embedder key/value | allowlisted ✅ |
| 31 | `SetWindowUserTitle` | pickle: `i32 window_id`, `string` | user-named window | allowlisted ✅ |
| 32 | `SetWindowVisibleOnAllWorkspaces` | struct, 8 B: `i32 window_id`, `bool` + 3 B pad | | allowlisted ✅ |
| 33 | `AddTabExtraData` | pickle: `i32 tab_id`, `string key`, `string data` | | allowlisted ✅ |
| 34 | `AddWindowExtraData` | pickle: `i32 window_id`, `string key`, `string data` | | allowlisted ✅ |
| 35 | `SetPlatformSessionId` | pickle: `string` (no id prefix) | platform session handle | allowlisted ✅ |
| 36 | `SetSplitTab` | struct, 32 B, same shape as 25 | split-view membership | allowlisted ✅ |
| 37 | `SetSplitTabData` | pickle: token, `double ratio`, `string layout` | split-view geometry | allowlisted ✅ |
| 255 | `InitialStateMarker` | empty (`size == 1`) | end of initial-state block (§2.3) | special-cased ⚠️ |

IDs 3 and 4 are unassigned in this table and have never been used by the
session service. **[inferred]** — they are simply absent from the constant
block.

### 2.2 `Tabs_*` command IDs — a completely different table

**[chromium-source]** `components/sessions/core/tab_restore_service_impl.cc`,
lines 116–132, and the `switch` in
`TabRestoreServiceImpl::PersistenceDelegate::CreateEntriesFromCommands`.

| ID | Name | Payload |
|---|---|---|
| 1 | `UpdateTabNavigation` | pickle: `i32 tab_id`, then a navigation entry |
| 2 | `RestoredEntry` | struct, 4 B: `i32 entry_id` — the entry was reopened, remove it |
| 3 | `WindowDeprecated` | struct, 12 B or 20 B (two historical shapes) |
| 4 | `SelectedNavigationInTab` | struct, 16 B: `i32 tab_id`, `i32 index`, `i64 timestamp` — **starts a tab** |
| 5 | `PinnedState` | struct, 1 B; only written when pinned, payload ignored |
| 6 | `SetExtensionAppID` | pickle: `i32 tab_id`, `string` |
| 7 | `SetWindowAppName` | pickle: `i32 window_id`, `string` |
| 8 | *SetTabUserAgentOverride* (obsolete) | pickle |
| 9 | `Window` | pickle: `i32 id`, `i32 selected_tab_index`, `i32 num_tabs`, `i64 ts`, `i32 x,y,w,h`, `i32 show_state`, `string workspace`, `i32 type` — **starts a window of `num_tabs` tabs** |
| 10 | `SetTabGroupData` | pickle: token, `string16 title`, `u32 colour`, `bool is_saved`, `string? guid`, `bool is_collapsed` |
| 11 | `SetTabUserAgentOverride2` | pickle |
| 12 | `SetWindowUserTitle` | pickle: `i32 window_id`, `string` |
| 13 | `CreateGroup` | pickle: token, `i32 id`, `i32 num_tabs`, `i32 browser_id`, `string16 title`, `u32 colour`, `bool is_saved`, `string? guid`, `i64 ts` |
| 14 | `AddTabExtraData` | pickle |
| 15 | `CreateSplit` | pickle: token, `i32 id`, `i64 ts`, `double ratio`, `i32 layout` |
| 16 | `SetTabSplitData` | pickle: token, `double ratio`, `i32 layout` |
| 255 | marker | empty |

Note how badly these collide. In a `Tabs_*` file ID 1 is 66 % of all commands
and is a pickled navigation; under the `Session_*` table ID 1 is an obsolete
window-bounds struct. ID 6 is a short app-id pickle in a `Tabs_*` file and a
navigation pickle in a `Session_*` file. **A parser must select its ID table
from the file-name prefix and must never fall back to the other one.**
**[observed-in-corpus]** + **[chromium-source]**

`Tabs_*` files are also *positional*: a `Window` (9) or `CreateGroup` (13)
command declares `num_tabs`, and the next `num_tabs` `SelectedNavigationInTab`
(4) commands are its tabs; navigation commands (1) attach to whichever tab
command was seen last. There are no tab ids threading the records together the
way there are in `Session_*`. **[chromium-source]** (`current_window`,
`current_group`, `current_tab` locals in `CreateEntriesFromCommands`.)

Also, in `Tabs_*` the navigation index written in a nav pickle is discarded and
replaced by its ordinal:
`current_tab->navigations.back().set_index(current_tab->navigations.size() - 1)`
— "When navigations are serialized, only `gMaxPersistNavigationCount`
navigations are written. This leads to inconsistent indices."
**[chromium-source]**

### 2.3 Command 255, the initial-state marker

`kInitialStateMarkerCommandId = 255` is reserved by the storage layer; the
service layer is forbidden from using it (`DCHECK_NE` in
`CommandStorageBackend::AppendCommands`). **[chromium-source]**

It is appended **after** the first batch of commands, not before:

```cpp
if (truncate) {
  CloseFile();
  commands.push_back(std::make_unique<SessionCommand>(kInitialStateMarkerCommandId, 0));
}
```
— `CommandStorageBackend::AppendCommands` **[chromium-source]**

So the marker means "everything before me is the complete rebuilt state of the
browser; everything after me is incremental". Chrome uses it only to decide
whether a file is worth reading at all
(`FindLastSessionFile` → `GetMarkerStatus` → `ReadToMarker`), and
`SessionFileReader::Read` **drops the marker command** from the returned list
while keeping every command on both sides of it. **[chromium-source]**

Measured positions **[observed-in-corpus]**:

| file | commands | marker at index |
|---|---|---|
| `Session_…181` | 2 855 | 2 634 |
| `Session_…321` | 2 698 | 2 670 |
| `Session_…609` | 1 036 | 990 |
| `Session_…472` | 810 | 622 |
| `Session_…198` | 911 | 748 |
| `Session_…553` | 1 132 | 980 |
| `Tabs_…493` | 255 | 88 |
| `Tabs_…857` | 256 | 87 |

Exactly one marker per file in all 8. Never at index 0 — the reference's error
string "Missing or unexpected initial session marker" describes it as an
*initial* marker, which is wrong about its position, though the count of one
happens to hold.

A missing marker means Chrome itself would skip the file, so it is a useful
*quality signal* — but a file without one still parses fine and still contains
tabs. Surface it; do not die on it. **[inferred]**

### 2.4 Window lifecycle: the `is_constrained` trick

A `SessionWindow` is constructed with `is_constrained(true)`
(`components/sessions/core/session_types.cc`, `SessionWindow::SessionWindow`),
and **only** `kCommandSetWindowType` (9) sets it to false
(`session_service_commands.cc`, `case kCommandSetWindowType`).
`SortTabsBasedOnVisualOrderAndClear` then discards every window that is
constrained or has no tabs. **[chromium-source]**

The consequence: **a window is real if and only if it received command 9.**
That is why `kCommandWindowClosed` (17) can simply `windows->erase(id)` even
though a later `GetWindow()` call would recreate the entry — the recreated
entry is constrained and gets discarded again. A parser that keeps a set of
"windows that saw command 9" minus "windows that saw command 17" reproduces
Chromium exactly. **[inferred from chromium-source]**

Window `type` values: `TYPE_NORMAL = 0`, `TYPE_POPUP = 1`, `TYPE_APP = 2`,
`TYPE_DEVTOOLS = 3`, `TYPE_APP_POPUP = 4`
(`session_types.h`, `SessionWindow::WindowType`). **[chromium-source]**
All 158 window-type commands in the corpus are `TYPE_NORMAL`.
**[observed-in-corpus]**

### 2.5 What the reference gets wrong, right, and dangerously

`reference/b_tabs_export.py` handles 0, 2, 5, 6, 7, 9, 11, 12, 16, 17, 24 and
allowlists `{8, 13, 14, 19, 20, 21, 23, 25, 27, 28, 29, 30, 31, 32, 33, 34, 35,
36, 37, 255}`. Anything else raises `ValueError`.

**Gets right.** The struct layouts for 0, 2, 7, 9, 12, 16, 17; the pickle
framing of command 6 including the alignment rule; and — verified line by line
against Chromium in §6 — the navigation-pruning arithmetic for 5, 11 and 24.

**Wrongly fatal.** Every ID outside its two lists: **1, 3, 4, 10, 15, 18, 22,
26**, plus any ID Chrome adds after 37. The one that matters today is **15,
`SetWindowAppName`** — written by
`SessionServiceBase::BuildCommandsForBrowser` for any window created with a
non-empty `app_name`, i.e. any installed PWA or `--app=` window
(`chrome/browser/sessions/session_service_base.cc`, lines 700–705)
**[chromium-source]**. A user with one PWA window open loses the entire
snapshot. IDs 10, 18, 22 and 26 are obsolete-but-still-read by Chromium, so
they can appear in files written by an older browser or carried through a
migration.

**Wrong in substance.** It ignores 25 and 27, which is where tab groups live
(§4), so `group` is silently always absent.

**Fatal where Chromium degrades.** Three more `raise`s with no equivalent in
Chromium: a truncated trailing command (§1.4), a tab whose selected navigation
index is not present (§6.4), and a tab with "incomplete window metadata" —
which in Chromium is simply a tab that gets dropped while the other 200 are
kept.

### 2.6 Measured frequencies

`Session_*`, 6 files, 9 442 commands total. **[observed-in-corpus]**

| ID | Name | Count | Share | Payload sizes seen |
|---:|---|---:|---:|---|
| 6 | `UpdateTabNavigation` | 2 774 | 29.38 % | variable, 192 B – ~5.9 KB |
| 2 | `SetTabIndexInWindow` | 1 109 | 11.75 % | 8 |
| 7 | `SetSelectedNavigationIndex` | 1 019 | 10.79 % | 8 |
| 21 | `LastActiveTime` | 1 002 | 10.61 % | 16 |
| 0 | `SetTabWindow` | 984 | 10.42 % | 8 |
| 19 | `SessionStorageAssociated` | 984 | 10.42 % | 48 |
| 32 | `SetWindowVisibleOnAllWorkspaces` | 299 | 3.17 % | 8 |
| 23 | `SetWindowWorkspace2` | 296 | 3.13 % | 1 964 – 2 012 |
| 14 | `SetWindowBounds3` | 243 | 2.57 % | 24 |
| 8 | `SetSelectedTabInIndex` | 177 | 1.87 % | 8 |
| 9 | `SetWindowType` | 158 | 1.67 % | 8 |
| 25 | `SetTabGroup` | 137 | 1.45 % | 32 |
| 12 | `SetPinnedState` | 131 | 1.39 % | 8 |
| 13 | `SetExtensionAppID` | 94 | 1.00 % | 44 |
| 20 | `SetActiveWindow` | 17 | 0.18 % | 4 |
| 16 | `TabClosed` | 9 | 0.10 % | 16 |
| 255 | marker | 6 | 0.06 % | 0 |
| 27 | `SetTabGroupMetadata2` | 3 | 0.03 % | 92 |

`Tabs_*`, 2 files, 511 commands total. **[observed-in-corpus]**

| ID | Name | Count | Share | Payload sizes |
|---:|---|---:|---:|---|
| 1 | `UpdateTabNavigation` | 339 | 66.34 % | variable, 184 B upwards |
| 4 | `SelectedNavigationInTab` | 125 | 24.46 % | 16 |
| 6 | `SetExtensionAppID` | 41 | 8.02 % | 44 |
| 2 | `RestoredEntry` | 4 | 0.78 % | 4 |
| 255 | marker | 2 | 0.39 % | 0 |

**Never observed in this corpus:** `Session_*` IDs 1, 3, 4, 5, 10, 11, 15, 17,
18, 22, 24, 26, 28, 29, 30, 31, 33, 34, 35, 36, 37. Note that this includes
**5, 11 and 24** — the three pruning commands the reference implements most
carefully have zero occurrences here. They are real (Chromium writes them from
`SessionService::TabNavigationPathPruned*`) but rare; they need unit tests
against synthetic bytes, because the corpus will not exercise them.
**[observed-in-corpus]** + **[inferred]**

Also note 17 (`WindowClosed`) never occurs while 16 (`TabClosed`) occurs 9
times. Do not assume both paths get exercised in testing.

Observed payload sizes confirm every struct layout in §2.1 including padding —
e.g. `SetTabGroup` at exactly 32 B and `LastActiveTime` at exactly 16 B are only
explicable with the C padding described there.

---

## 3. Pickle payload encoding

Variable-length commands wrap their payload in a `base::Pickle`.

### 3.1 The pickle header inside the command contents

`SessionCommand(id, const base::Pickle& pickle)` copies `pickle.size()` bytes,
and `Pickle::size()` is `header_size_ + payload_size` — **the 4-byte header is
included in the command contents**. `SessionCommand::ContentsAsPickle()` then
calls `base::PickleIterator::WithData(contents())`. **[chromium-source]**
`session_command.cc`; `base/pickle.h` (`Pickle::Header`, `Pickle::size`);
`base/pickle.cc` (`PickleIterator::WithData`).

```
command contents:
+---------------------------+- - - - - - - - - - - - - - - - - -+
| payload_size (uint32 LE)  |  payload (payload_size bytes)     |
+---------------------------+- - - - - - - - - - - - - - - - - -+
 <-------- 4 bytes --------> <--------- payload_size ---------->
```

`WithData` is subtler than it looks, and the subtlety is worth copying:

```cpp
if (header.payload_size > data.size() - sizeof(Pickle::Header))
  return PickleIterator();              // invalid: refuse
const size_t header_size = data.size() - header.payload_size;
if (header_size != bits::AlignUp(header_size, sizeof(uint32_t)))
  return PickleIterator();              // invalid: refuse
iter.reader_ = SpanReader(data.subspan(header_size));
```

That is: the payload is taken from the **end** of the buffer, and any slack is
skipped at the **front**. Normally `header_size == 4` and this is unremarkable.
If `payload_size` is smaller than `len(contents) - 4` it silently skips extra
leading bytes instead of failing. A parser that hardcodes "skip 4" will
disagree with Chromium in that (never-observed) case; a parser that computes
`len(contents) - payload_size` matches it. **[chromium-source]**

Measured: all 2 774 `UpdateTabNavigation` commands in the corpus have
`len(contents) - payload_size == 4` exactly. **[observed-in-corpus]**

### 3.2 Primitives and the alignment rule

**[chromium-source]** `base/pickle.cc`: `AlignAfterRead`,
`ReadBuiltinTypeAndAlign`, `ReadLengthAndAlign`, `ReadBytesAndAlign`,
`PickleIterator::ReadString`, `ReadString16`, `ReadBool`, `ReadInt`;
`Pickle::WriteString`, `WriteString16`, `WriteData`,
`ClaimUninitializedBytesInternal`.

**The rule, stated once:** after reading or writing *any* item of `n` bytes, the
cursor advances to `align_up(n, 4)`. Padding is written as zeroes. Because every
primitive ends 4-aligned, the cursor is always at a multiple of 4 relative to
the start of the payload.

| primitive | bytes consumed | then padded to |
|---|---|---|
| `bool` | 1 | 4 |
| `int` / `uint32` | 4 | 4 |
| `int64` / `uint64` / `double` | 8 | 8 (already aligned) |
| `string` | 4 (`int32` byte count `n`) + `n` | `4 + align_up(n, 4)` |
| `string16` | 4 (`int32` **character** count `n`) + `2n` | `4 + align_up(2n, 4)` |

Note the asymmetry that catches everyone: **`string` counts bytes, `string16`
counts UTF-16 code units.** The byte length of a `string16` is `2n`.

- `string` is a raw byte string. Chromium writes `std::string` here, which for
  URLs, GUIDs, workspace ids and extension ids is UTF-8 (a URL spec is ASCII by
  construction). Chromium does not validate the encoding on read, so a decoder
  should be lossy rather than strict. **[chromium-source]** + **[inferred]**
- `string16` is UTF-16, native endian, i.e. **UTF-16LE** on every desktop
  target. Unpaired surrogates are possible: Chromium neither validates nor
  normalises. Decode lossily. **[chromium-source]** + **[inferred]**
- A negative length is rejected outright (`ReadLengthAndAlign` skips to end and
  returns false when `result_int < 0`). Treat a negative length as a malformed
  record, not as a truncated file. **[chromium-source]**
- `string16` length is bounds-checked against multiplication overflow
  (`CheckMul(len, sizeof(char16_t))`). A parser must do the same before
  allocating. **[chromium-source]**

### 3.3 Worked diagram: command 6, `UpdateTabNavigation`

Field order from `SerializedNavigationEntry::WriteToPickle`
(`components/sessions/core/serialized_navigation_entry.cc`) preceded by the tab
id from `CreateUpdateTabNavigationCommand`
(`components/sessions/core/base_session_service_commands.cc`).
**[chromium-source]**

```
contents
 0  +---------------------------+  payload_size (uint32 LE) = len(contents) - 4
 4  +---------------------------+  tab_id                   (int32)
 8  +---------------------------+  navigation index         (int32)
12  +---------------------------+  url length U             (int32, bytes)
16  +===========================+  url bytes                (U bytes, UTF-8)
    |  ... U bytes ...          |
    +---------------------------+  zero padding to align_up(U, 4)
T   +---------------------------+  title length C           (int32, UTF-16 units)
T+4 +===========================+  title bytes              (2C bytes, UTF-16LE)
    |  ... 2C bytes ...         |
    +---------------------------+  zero padding to align_up(2C, 4)
    +---------------------------+  encoded_page_state       (string, often large)
    +---------------------------+  transition_type          (int32)
    +---------------------------+  type_mask                (int32)
    +---------------------------+  referrer_url             (string)
    +---------------------------+  obsolete referrer policy (int32, always 2)
    +---------------------------+  original_request_url     (string)
    +---------------------------+  is_overriding_user_agent (bool + 3 pad)
    +---------------------------+  timestamp                (int64)
    +---------------------------+  removed search_terms     (string16, empty)
    +---------------------------+  http_status_code         (int32)
    +---------------------------+  referrer_policy          (int32)
    +---------------------------+  extended_info count N    (int32)
    +---------------------------+  N × (string key, string value)
    +---------------------------+  task_id, parent_task_id, root_task_id (3 × int64)
    +---------------------------+  0                        (int32, legacy child count)

where  T = 16 + align_up(U, 4)
```

For knowmoretabs only the first four fields matter (`tab_id`, `index`, url,
title). Everything after the title is skippable — and *should* be skipped rather
than parsed, because `encoded_page_state` is an opaque blob that dominates the
record size and every field after it has been added at a different time.
`ReadFromPickle` treats all of them as optional: it hard-fails only on the first
five, then uses `std::ignore` and default values for the rest, precisely so old
files keep working. **[chromium-source]**

Corpus check: 2 774/2 774 navigation records parse with these rules, 0 decode
failures. URL byte lengths mod 4 are spread `{0: 945, 1: 541, 2: 634, 3: 654}`,
so the padding path is thoroughly exercised, not a theoretical concern.
**[observed-in-corpus]**

### 3.4 Tokens

`base::Token` is 128 bits written as two `uint64`s, high first:

```cpp
void WriteTokenToPickle(Pickle* pickle, const Token& token) {
  pickle->WriteUInt64(token.high());
  pickle->WriteUInt64(token.low());
}
```
— `base/token.cc` **[chromium-source]**

Both tab-group ids and split-tab ids are tokens. Render as a 32-hex-digit
string, `high` then `low`, to match Chromium's `Token::ToString`.

---

## 4. Tab groups

**Yes. Group id, title and colour are all recoverable from `Session_*`
commands alone. No companion file is required, and nothing group-related lives
in `Local State` or `Preferences`.** **[chromium-source]** +
**[observed-in-corpus]**

### 4.1 Command 25 — membership

`TabGroupPayload` is a raw struct, so the wire layout is the C layout with
padding **[chromium-source]** (`session_service_commands.cc`, `struct
TabGroupPayload`, `CreateTabGroupCommand`):

```cpp
struct SerializedToken { uint64_t id_high; uint64_t id_low; };
struct TabGroupPayload {
  SessionID::id_type tab_id;   // int32
  SerializedToken maybe_group;
  bool has_group;
};
```

```
 0 +-------------------+  tab_id      (int32 LE)
 4 +-------------------+  PADDING     (4 bytes, for uint64 alignment)
 8 +-------------------+  id_high     (uint64 LE)
16 +-------------------+  id_low      (uint64 LE)
24 +-------------------+  has_group   (1 byte: 0 or 1)
25 +-------------------+  PADDING     (7 bytes)
32                        total
```

All 137 command-25 payloads in the corpus are exactly 32 bytes, which confirms
the padding. **[observed-in-corpus]**

Semantics: last write wins per `tab_id`. `has_group == false` means *explicitly
ungrouped* — Chrome writes 25 with `has_group = false` when a tab leaves a
group, so the command appears far more often than there are grouped tabs. In one
corpus file, 61 command-25 records cover 33 distinct tabs of which 2 end up
grouped. **[observed-in-corpus]**

Chrome deliberately does **not** write this command while a window is closing,
"so tabs should stay in their groups"
(`chrome/browser/sessions/session_service.cc`, `SessionService::SetTabGroup`).
**[chromium-source]**

### 4.2 Command 27 — title, colour, collapsed, saved id

A pickle **[chromium-source]** (`CreateTabGroupMetadataUpdateCommand` writes,
`case kCommandSetTabGroupMetadata2` reads):

```
 0 +-------------------+  payload_size            (uint32 LE)
 4 +-------------------+  group token high        (uint64 LE)
12 +-------------------+  group token low         (uint64 LE)
20 +-------------------+  title length C          (int32, UTF-16 units)
24 +===================+  title                   (2C bytes UTF-16LE)
   +-------------------+  pad to align_up(2C, 4)
   +-------------------+  colour                  (uint32, TabGroupColorId)
   +-------------------+  is_collapsed            (bool + 3 pad)   -- added M88
   +-------------------+  is_saved                (bool + 3 pad)   -- added M113
   +-------------------+  saved_guid              (string)  only if is_saved
```

The trailing fields are genuinely optional on read — Chromium uses
`std::ignore = iter.ReadBool(...)` for `is_collapsed` and `is_saved`, so a file
written before M88/M113 simply stops early and the defaults stand. Copy that:
read what is there, stop at the first short read, keep what you got.
**[chromium-source]**

Colour is `tab_groups::TabGroupColorId` (`components/tab_groups/
tab_group_color.h`), and the header says outright: "Do not change or reuse the
values… any code that reads an enum value from disk should check it against the
map from `GetTabGroupColorLabelMap()`. If the value is not contained in the
map's keys, default to `kGrey`." **[chromium-source]**

| value | colour |
|---:|---|
| 0 | grey |
| 1 | blue |
| 2 | red |
| 3 | yellow |
| 4 | green |
| 5 | pink |
| 6 | purple |
| 7 | cyan |
| 8 | orange |

Follow the instruction: unknown value → grey, never an error.

`saved_guid` is a UUID pointing into the Saved/Synced Tab Groups store
(`TabGroupSyncService`), which is a *different* database entirely. We do not
need it and should not chase it — the title and colour are already in the
session file. Keep it if a future feature wants group identity across devices.
**[chromium-source]** + **[inferred]**

### 4.3 Is the metadata actually present when you need it?

Yes — because group metadata is part of the **initial-state rebuild**, not only
of live edits. `SessionServiceBase::BuildCommandsForBrowser` iterates
`group_model->ListTabGroups()` and emits a command 27 for every group in every
window before it emits any tab commands
(`chrome/browser/sessions/session_service_base.cc`, lines 729–755).
**[chromium-source]**

Corpus confirmation **[observed-in-corpus]**:

| file | tabs grouped | distinct group tokens | command 27 records | groups with title+colour | cmd 27 before the marker? |
|---|---:|---:|---:|---:|---|
| `Session_…181` | 2 | 1 | 1 | 1/1 | yes (idx 402 < 2 634) |
| `Session_…321` | 2 | 1 | 1 | 1/1 | yes (idx 384 < 2 670) |
| `Session_…609` | 1 | 1 | 1 | 1/1 | yes (idx 394 < 990) |
| `Session_…472` | 0 | 0 | 0 | — | — |
| `Session_…198` | 0 | 0 | 0 | — | — |
| `Session_…553` | 0 | 0 | 0 | — | — |

Three groups existed in the corpus; all three had recoverable metadata, all
three appeared in the initial-state block, all three carried a non-empty title,
a valid colour (grey and orange were the values present), `is_collapsed =
false`, `is_saved = true` and a 36-character `saved_guid`, with zero trailing
bytes left over. The parse consumed each 92-byte payload exactly.

### 4.4 Reconstruction algorithm

1. Fold commands 25 into `tab_id → Option<token>` (last write wins).
2. Fold commands 27 into `token → {title, colour, collapsed, saved_guid}` (last
   write wins).
3. A tab's group is `groups.get(membership[tab_id])`.
4. If a tab references a token with no metadata record, still emit the group
   with its id and `title: null, colour: "grey"`. Chromium does exactly this —
   `AddTabsToWindows` pushes a bare `SessionTabGroup(group_id)` when the map has
   no entry. **[chromium-source]**
5. Groups whose tabs have all gone are dangling: "Since we don't have explicit
   commands for opening and closing tab groups, there may be dangling
   `SessionTabGroup` entries after all tabs in a group are closed."
   (`AddTabsToWindows`) **[chromium-source]** Drop any group with no surviving
   tabs.
6. A group never spans windows — "We rely on the fact that tab groups and split
   tabs can't be split between windows." (`AddTabsToWindows`)
   **[chromium-source]**

`Tabs_*` also carries group data (IDs 10 and 13), but those describe *closed*
groups in the recently-closed list, which is a different thing from the groups
a user currently has open. For knowmoretabs, read groups from `Session_*`.
**[inferred]**

Command 27 is 0.03 % of commands. It is the highest value-per-byte record in the
whole format, and the reference throws all of it away.

---

## 5. The files: which one to read

### 5.1 Current layout (Chrome 85 and later)

Files live in `<profile>/Sessions/`, named `<prefix>_<timestamp>`.
**[chromium-source]** `components/sessions/core/session_constants.cc`:

```cpp
kSessionsDirectory          = "Sessions";            // "Added in Chrome 85."
kEncryptedSessionsDirectory = "Sessions_Encrypted";  // "Added in Chrome 148 for crbug.com/479420496."
kTabSessionFileNamePrefix   = "Tabs";                // "Added in Chrome 85."
kSessionFileNamePrefix      = "Session";             // "Added in Chrome 85."
kAppSessionFileNamePrefix   = "Apps";                // "Added in Chrome 91."
kTimestampSeparator         = "_";
```

| prefix | `SessionType` | contents |
|---|---|---|
| `Session_` | `kSessionRestore` | **the open windows and tabs** — what we want |
| `Tabs_` | `kTabRestore` | the recently-closed list (Ctrl+Shift+T history) |
| `Apps_` | `kAppRestore` | app/PWA windows, same command table as `Session_` |

**Read `Session_*`.** `Tabs_*` is a different ID table (§2.2) describing tabs
the user has already closed. `Apps_*` uses the `Session_*` table and could be
read as a bonus source of PWA windows; it is not needed for the 80/20.
**[chromium-source]** + **[inferred]**

### 5.2 The numeric suffix

`TimestampToString` is
`base::NumberToString(time.ToDeltaSinceWindowsEpoch().InMicroseconds())` — a
decimal count of **microseconds since 1601-01-01 UTC** (the Windows epoch), on
every platform. **[chromium-source]** `command_storage_backend.cc`,
`TimestampToString`, `TimestampFromPath`.

```
unix_seconds = suffix / 1_000_000 - 11_644_473_600
```

Sanity check against the corpus **[observed-in-corpus]**:

| suffix | decodes to (UTC) | file mtime (local, UTC+7) |
|---|---|---|
| `13433056602603321` | 2026-09-05T04:36:42Z | 2026-09-05 11:37 |
| `13434341530659553` | 2026-09-20T01:32:10Z | 2026-09-20 08:42 |

The suffix is the moment the log was **created** (`TruncateOrOpenFile` sets
`timestamp_` then derives the path from it); mtime is the last append. So the
suffix always precedes mtime, by seconds or by hours depending on how long the
browser has been running. Sort by suffix, but report `saved_at` from mtime —
that is the last moment the file reflected reality. **[chromium-source]** +
**[inferred]**

Parsing rule, mirroring `TimestampFromPath`: split the basename on `_`, require
exactly two parts, require the second to parse as `int64`. Anything else is not
a session file. **[chromium-source]**

Chrome guarantees the suffix is strictly increasing — `TruncateOrOpenFile`
bumps it by 1 µs if the clock went backwards or the value would repeat — so
**"newest file" = largest suffix**, and this is more reliable than mtime.
**[chromium-source]**

Do not assume a fixed number of files. Chromium keeps "current" and "last" and
best-effort deletes the rest (`DeleteLastSessionFiles`), but deletion is best
effort and the encrypted/cleartext dual-write adds more. This machine's
`Default` profile currently holds 3 × `Session_`, 3 × `Tabs_`, 1 × `Apps_` in
`Sessions/`. **[observed-in-corpus]**

Also, "newest" is not automatically "valid": Chromium's `FindLastSessionFile`
walks files newest-first and takes the first one whose marker can be reached,
skipping the rest. A robust tool should do the same — try the newest, and on a
bad header fall back to the next. **[chromium-source]**

### 5.3 `Current Session` / `Last Session` / `Current Tabs` / `Last Tabs`

The pre-Chrome-85 layout: four fixed-name files directly in the profile
directory, no `Sessions/` subdirectory, no timestamp suffix. `Current Session`
and `Current Tabs` were the live logs; on a clean shutdown
`MoveCurrentSessionToLastSession` renamed them to `Last Session` / `Last Tabs`.
The *container and command formats were the same* — only the naming and
location changed. **[inferred]** (the rename semantics survive as
`CommandStorageBackend::MoveCurrentSessionToLastSession` **[chromium-source]**;
the file-name migration is corroborated by the Chrome 85/86 user reports on
[chromium-discuss](https://groups.google.com/a/chromium.org/g/chromium-discuss/c/O33rONHw-g0)
and the older format description at
[Digital Investigation](https://digitalinvestigation.wordpress.com/2012/09/03/chrome-session-and-tabs-files-and-the-puzzle-of-the-pickle/)).

Those files would also be version 1, which Chromium dropped in 2021. Any tool
built now should look only in `Sessions/`; if it finds `Current Session`
instead, that profile is a decade old and the right answer is a clear message,
not a parser. **[inferred]**

### 5.4 Platform roots (for slice 5)

Not in Chromium source; recorded here so slice 5 does not have to rediscover it.
**[inferred]**

| OS | Chrome user-data root |
|---|---|
| macOS | `~/Library/Application Support/Google/Chrome` |
| Linux | `~/.config/google-chrome` (Chromium: `~/.config/chromium`) |
| Windows | `%LOCALAPPDATA%\Google\Chrome\User Data` |

Profile directory under that root: `Default`, `Profile 1`, `Profile 2`, …
`Local State` → `profile.last_used` names the most recently used one, which is
what the reference does and is a good default.

---

## 6. Navigation-entry pruning (commands 5, 11, 24)

### 6.1 The model

A tab holds a list of `SerializedNavigationEntry`, each carrying its own
`index()` — the index in the live `NavigationController`, **not** its position
in the list. Chromium keeps the list sorted by that index, sparse, and only
writes entries within `gMaxPersistNavigationCount = 6` either side of the
current one (`session_constants.cc`; `SessionServiceBase::BuildCommandsForTab`
computes `min_index = max(current - 6, 0)`, `max_index = min(current + 6,
count)`). **[chromium-source]**

Two helpers do all the work **[chromium-source]**
(`session_service_commands.cc`):

```cpp
// Returns the first navigation with index >= |index|, else end().
FindClosestNavigationWithIndex(navigations, index)

void ProcessTabNavigationPathPrunedCommand(payload, tab) {
  // 1. fix up the selected index
  if (cur >= payload.index && cur < payload.index + payload.count)
    cur = payload.index - 1;
  else if (cur >= payload.index + payload.count)
    cur = cur - payload.count;
  // (no change if cur < payload.index)

  // 2. erase the range
  tab->navigations.erase(
      FindClosestNavigationWithIndex(&nav, payload.index),
      FindClosestNavigationWithIndex(&nav, payload.index + payload.count));

  // 3. renumber what survives
  for (auto& entry : tab->navigations)
    if (entry.index() >= payload.index)
      entry.set_index(entry.index() - payload.count);
}
```

### 6.2 Command 5 — `TabNavigationPathPrunedFromBack`

Payload `{ i32 tab_id, i32 index }`. Chromium:

```cpp
tab->navigations.erase(FindClosestNavigationWithIndex(&nav, payload.index),
                       tab->navigations.end());
```

**Effect:** delete every navigation whose index ≥ `index`. Surviving indices are
unchanged. **The selected index is deliberately not touched.** This is the
"forward history was discarded because the user navigated somewhere new" case;
the selected entry is by definition before the cut. **[chromium-source]**

### 6.3 Command 11 — `TabNavigationPathPrunedFromFront`

Payload `{ i32 tab_id, i32 index }`, where `index` is a **count**, not a
position. Chromium validates `index > 0`, then rewrites it as
`{ index: 0, count: prune_front_payload.index }` and calls the shared helper.
**[chromium-source]**

**Effect:** delete the first `count` navigations (indices `0 .. count-1`), shift
every survivor down by `count`. Selected index: if it was inside the removed
range it becomes `-1` (`payload.index - 1` where `payload.index == 0`);
otherwise it drops by `count`.

A selected index of `-1` is a real, intended outcome, not an error. §6.5 covers
what to do with it.

### 6.4 Command 24 — `TabNavigationPathPruned`

Payload `{ i32 tab_id, i32 index, i32 count }`, validated `index >= 0 && count >
0`, then straight into the shared helper. This supersedes both 5 and 11;
Chromium still reads all three because old files exist. **[chromium-source]**

### 6.5 Is the reference's arithmetic correct?

**Yes.** Line by line:

```python
elif command in (5, 11, 24):
    identifier, start = struct.unpack_from('<ii', payload)
    state = tab(identifier)
    if command == 5:
        state['navigation'] = {i: n for i, n in state['navigation'].items() if i < start}
        continue
    count = start if command == 11 else struct.unpack_from('<i', payload, 8)[0]
    start = 0 if command == 11 else start
    end = start + count
    selected = state['selected']
    if start <= selected < end:
        state['selected'] = start - 1
    elif selected >= end:
        state['selected'] -= count
    state['navigation'] = {
        i if i < start else i - count: n
        for i, n in state['navigation'].items() if not start <= i < end
    }
```

| Chromium | reference | verdict |
|---|---|---|
| cmd 5: erase index ≥ `index`, leave selected alone | keeps `i < start`, `continue`s before touching selected | ✅ identical |
| cmd 11: `index := 0`, `count := payload.index` | `count = start`, `start = 0` | ✅ identical |
| selected inside `[index, index+count)` → `index - 1` | `start - 1` | ✅ identical |
| selected ≥ `index+count` → `selected - count` | `selected -= count` | ✅ identical |
| selected < `index` → unchanged | falls through both branches | ✅ identical |
| erase `[closest(index), closest(index+count))` on a sorted list | dict filter `not start <= i < end` | ✅ equivalent — the iterator pair spans exactly the entries whose index lies in `[index, index+count)` |
| renumber entries with `index >= payload.index` by `-count` | `i if i < start else i - count` | ✅ identical (after the erase the two sets coincide) |

Two divergences, both harmless in practice:

- The reference skips Chromium's validation (`index > 0` for cmd 11, `index >=
  0 && count > 0` for cmd 24). With `count == 0` the reference's transform is a
  no-op; with a negative count it would produce nonsense. Chromium *aborts the
  entire restore* on invalid values — returning from `CreateTabsAndWindows`
  mid-loop — which is worse. Validate and **skip the one command**.
- Chromium reads `payload.index` as the range start for cmd 5 and never adjusts
  the selected index; if the selected index happened to be ≥ the cut, Chromium
  leaves it dangling and relies on the clamp below. The reference does the same.
  Matching a Chromium quirk is the right call here.

### 6.6 The selected index is not an index into the list

This is the subtlety the reference gets wrong, and it is a *semantic* error, not
an arithmetic one. `SessionTab::current_navigation_index` is an index in the
navigation-controller's numbering. Chromium resolves it to a list position at
the very end, in `AddTabsToWindows` **[chromium-source]**:

```cpp
auto j = FindClosestNavigationWithIndex(&nav, tab->current_navigation_index);
if (j == nav.end())
  tab->current_navigation_index = nav.size() - 1;   // clamp to last
else
  tab->current_navigation_index = j - nav.begin();  // nearest at or after
```

and `SessionTab::normalized_navigation_index()` clamps again:

```cpp
return std::max(0, std::min(current_navigation_index,
                            static_cast<int>(navigations.size() - 1)));
```

with the header comment spelling out why: "this value can be larger than the
size of `|navigations|`, due to only valid url's being stored (ie
chrome://newtab is not stored). Bounds checking must be performed."
**[chromium-source]** `session_types.h`.

So the correct lookup is:

1. If no navigations at all → the tab has no URL. Chromium skips it entirely
   (`if (!tab->window_id.id() || tab->navigations.empty()) continue;`).
2. Otherwise take the first navigation with `index >= selected`.
3. If there is none, take the **last** navigation.

Never an error. The reference's
`raise ValueError(f'Tab {identifier} has no navigation at its selected index')`
has no counterpart anywhere in Chromium.

Measured: across all 6 `Session_*` files (973 live tabs — 290, 289, 115, 74, 92,
113), the selected index matched a navigation exactly **973 times out of 973**;
the fallback never fires, and no tab had zero navigations.
**[observed-in-corpus]** That is expected: the rebuild writes the ±6
window *centred on* the current index, so a freshly-rebuilt tab always has its
selected entry. The fallback exists for tabs whose history was pruned live after
the rebuild, i.e. exactly the long-running sessions we care about. Implement it;
do not expect the corpus to prove it.

---

## 7. Version and encryption risk

### 7.1 Encryption is not hypothetical. It is shipping right now.

`kEncryptSessionStorage` is **enabled by default** in Chromium `main`
**[chromium-source]** (`components/sessions/core/command_storage_features.cc`):

```cpp
BASE_FEATURE(kEncryptSessionStorage, base::FEATURE_ENABLED_BY_DEFAULT);
BASE_FEATURE_PARAM(std::string, kEncryptSessionStorageStageParam,
                   &kEncryptSessionStorage, "stage",
                   "write_both_read_only_clear");
```

The rollout is staged (`EncryptSessionStorageStage` in
`command_storage_features.h`, tracking crbug.com/479420496):

| stage | writes `Sessions/` (v3) | writes `Sessions_Encrypted/` (v5) | Chrome reads |
|---|---|---|---|
| `kClearOnly` | ✅ | ✖ (directory deleted) | cleartext |
| **`kWriteBothReadOnlyClear`** ← **default today** | ✅ | ✅ | cleartext |
| `kWriteBothReadPreferEncrypted` | ✅ | ✅ | encrypted, cleartext fallback |
| `kWriteEncryptedReadPreferEncrypted` | ✖ | ✅ | encrypted |

Only the last stage takes cleartext away. At that point
`ShouldWriteCleartextFiles()` returns false and `DeleteLastSessionFiles()`
removes the stale cleartext files. **[chromium-source]**

**Verified on this machine, today** **[observed-in-corpus]**:

```
Chrome 153.0.8010.48 (Stable), macOS

<profile>/Default/Sessions/            Session_<ts>  53 4e 53 53 03 00 00 00
                                       Tabs_<ts>     53 4e 53 53 03 00 00 00
                                       Apps_<ts>     53 4e 53 53 03 00 00 00
<profile>/Default/Sessions_Encrypted/  Session_<ts>  53 4e 53 53 05 00 00 00
                                       Tabs_<ts>     53 4e 53 53 05 00 00 00
```

(Only the 8-byte headers of live-profile files were read, to establish the
version numbers. No live-profile file was parsed; the corpus analysis is
confined to the 8 authorised files.)

`Sessions_Encrypted/` exists and contains version-5 files. `Sessions/` is still
written and still version 3. Chrome 148 reached stable on 2026-05-05; current
stable is Chrome 153. So the dual-write stage has been live for roughly four
months and the encrypted directory is already on ordinary users' disks.

### 7.2 What happens when the last stage ships

Version 5 uses `os_crypt_async::Encryptor`, which on macOS means a key held in
the login Keychain under Chrome's own ACL, on Windows DPAPI plus App-Bound
Encryption, and on Linux the desktop keyring or a hardcoded fallback. Reading it
from a separate process is, by design, awkward to impossible — on Windows
App-Bound Encryption exists specifically to make it impossible for another
process running as the same user.

Realistic forward-compat story, in the order it will matter:

1. **Now → the flip.** Cleartext v3 keeps working. Nothing to do.
2. **At the flip.** `Sessions/` stops being written. A tool that only reads
   `Sessions/` sees an increasingly stale file, then nothing. **This is the
   failure mode to guard against: silently stale data, not a crash.** A parser
   that finds `Sessions_Encrypted/` with a newer timestamp than anything in
   `Sessions/` should say so loudly.
3. **After the flip.** Practical options, in descending order of sanity:
   - A browser extension. Already on the slice matrix as the `live-tabs` future
     row, already noted there as the thing that survives a format change but not
     a crash. Encryption is the argument that promotes it.
   - Chrome's own `--restore-last-session` / DevTools surfaces. Awkward.
   - Reimplementing `os_crypt_async` key derivation per platform. Possible on
     macOS and Linux, hostile on Windows, and it makes us a credential-reading
     tool, which is not what this is.

The design response is architectural, not defensive: keep the session-file
reader behind a source abstraction so a second source can be added beside it.
The brief already requires this for `live-tabs`; encryption makes it the
critical path rather than a nicety.

### 7.3 Other moving parts

The format changes in small, additive ways, roughly every few milestones
**[chromium-source]**:

| when | change |
|---|---|
| M88 | `is_collapsed` appended to command 27 |
| M104 | window `type` appended to `Tabs_*` command 9 |
| M113 | `is_saved` + `saved_guid` appended to command 27 |
| M126 | saved-group id appended to `Tabs_*` command 10 |
| M148 | `Sessions_Encrypted/`, file version 5 |
| M152 | `is_collapsed` appended to `Tabs_*` command 10 |
| recent | IDs 35 (`SetPlatformSessionId`), 36/37 (split tabs) added |

Every one of these is **appended to the end of an existing pickle** or is a
**new command id**. That is the whole pattern, and it is what forward
compatibility should be designed around:

- Unknown command id → skip, count, continue. Chromium's own reader is the
  outlier here (it `return`s and abandons the rest of the file); the *comment*
  above that code says it intends to "attempt to restore what we we can", and
  our requirements are weaker than a browser's, so skipping is both safer and
  more useful.
- Short pickle → keep the fields you got, default the rest. This is exactly
  what `std::ignore = iter.ReadBool(...)` does throughout.
- Unknown enum value → clamp to the documented default (grey for colours,
  normal for show state).

`// NEXT_VERSION = 6` tells you the file version will move again. Treat an
unknown version as "unsupported, here is what I saw", not as corruption.
**[chromium-source]**

---

## 8. Recommended parsing strategy

### 8.1 Fatal — and this is the complete list

Only three things should end a parse. Each is a condition under which no tab can
possibly be recovered.

1. **The file cannot be read.** I/O error, or fewer than 8 bytes.
2. **Bad signature.** First four bytes are not `53 4e 53 53`.
3. **Unsupported version.** Not 3. Version 5 gets its own error type with a
   specific message about encryption (§7), *not* a generic parse failure — the
   user needs to know their browser changed, not that our parser is broken.

Note what is deliberately absent: nothing about command ids, payload sizes,
tab/window consistency, the marker, or the tail. If the header is good, the
parse returns a result.

### 8.2 Skip and count

Each of these increments a counter that ends up in `Stats` and is reported.

| situation | action | counter |
|---|---|---|
| unknown command id | skip the command | `unknown_commands` (with a per-id breakdown) |
| known id, wrong payload size | skip the command | `malformed_commands` |
| pickle ends mid-field | keep the fields read so far, discard the rest of that command | `malformed_commands` |
| negative or overflowing length in a pickle | skip the command | `malformed_commands` |
| `payload_size` inconsistent with contents length | skip the command | `malformed_commands` |
| pruning command failing Chromium's validation (`count <= 0`, `index < 0`) | skip that command only — **not** the file | `malformed_commands` |
| torn tail (§1.4) | stop parsing, keep everything before it | `truncated_bytes` |

### 8.3 Degrade gracefully

| situation | behaviour |
|---|---|
| selected navigation index has no exact match | nearest navigation with `index >= selected`, else the last one (§6.6) |
| tab has no navigations at all | drop that tab, count it — Chromium does the same |
| tab references a window that never saw command 9 | drop that tab, count it (§2.4) |
| tab has no `SetTabIndexInWindow` | keep it, position `-1`, sort it last — the `Tab.position` field is presentational |
| tab references a group with no metadata | emit the group with its id, `title: null`, `colour: "grey"` |
| group has no surviving tabs | drop the group |
| unknown colour value | grey |
| unknown window type | treat as normal, but keep the raw number |
| no marker, or more than one | parse anyway, set a `marker_ok: false` flag |
| `Tabs_*` or `Apps_*` passed to `--session` | `Apps_*` parses with the `Session_*` table; `Tabs_*` must be refused with a message naming the right file, never parsed with the wrong table (§2.2) |

### 8.4 Order of operations

Single pass, then resolve. The command log is a fold, not a tree.

```
1. header           -> version, or fatal
2. fold commands    -> tabs{}, windows{}, groups{}, closed_tabs, closed_windows
                       (last write wins per key; 16 and 17 erase)
3. resolve windows  -> keep windows that saw cmd 9 and were not closed by cmd 17
4. resolve tabs     -> drop tabs with no navigations, or in a dropped window
5. resolve nav      -> selected index -> URL + title by the §6.6 rule
6. resolve groups   -> attach metadata, drop dangling groups
7. order            -> window by id ascending (== creation order), tab by
                       (tab_visual_index, tab_id) -- Chromium's
                       TabVisualIndexSortFunction and WindowOrderSortFunction
```

Window ordering is by window id because "window ids increment for each new
window, [so] this effectively sorts by creation time"
(`WindowOrderSortFunction`). Tab ties break on tab id
(`TabVisualIndexSortFunction`). **[chromium-source]** Matching this gives stable
output across snapshots, which is what the "skip an unchanged snapshot" check
depends on.

### 8.5 Statistics to surface

These populate `Stats` in `snapshot.json` and the one-line summary on stdout.

```
commands             total commands framed
commands_by_id       map id -> count           (the forward-compat tripwire)
unknown_commands     count, plus the unknown ids themselves
malformed_commands   known id, unusable payload
truncated_bytes      unparsed bytes at the tail (0 is the normal case)
marker_ok            exactly one command 255 was found
file_version         3, so that a future 5 is visible in the archive
tabs                 tabs emitted
tabs_dropped         with reasons: no_navigations, no_window, window_closed
windows              windows emitted
groups               groups emitted
groups_without_metadata
navigation_fallbacks how often §6.6's nearest-match rule fired
```

Human default: one line — `113 tabs across 12 windows, 3 groups`. Add
`(2 unknown commands, 48 trailing bytes)` only when those are non-zero, so a
format change is visible without being noisy. Under `--json`, all of it.

`commands_by_id` is the single most valuable field in the archive. Because
snapshots are immutable, a year of them is a longitudinal record of Chrome's
format; the day an unknown id appears is the day we learn about it, from data we
already saved.

### 8.6 Testing notes

The corpus will not exercise everything. Specifically it has **no** occurrences
of commands 5, 11, 24, 17, 15, 28–31, 33–37, no torn tails, no version-5 file,
no non-normal window type, and no case where the selected-index fallback fires.
All of these need synthetic fixtures. Build them by *writing* SNSS bytes from
the layouts above rather than by redacting real files — synthetic fixtures are
also the only kind this repo is allowed to commit.

The eight real files remain the only proof that the common path works on real
Chrome output. A parser that reproduces 289/289, 115/115, 74/74, 92/92 and
113/113 tabs on them is matching the reference implementation exactly; that
equivalence was confirmed while researching this document.
**[observed-in-corpus]**
