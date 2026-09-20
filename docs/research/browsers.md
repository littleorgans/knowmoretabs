# Chromium-family session and profile paths

This file is the path table for slices 4 (`browsers`) and 5 (`platforms`).
Implementation should copy these paths, not rediscover them.

Every path claim is tagged:

- **[verified-on-this-machine]** — `test -d` / directory listing on the
  author's macOS checkout, 2026-09-20. Existence and filename prefixes
  only. No session payloads, no profile display names, no URLs.
- **[documented]** — official vendor docs, Chromium source, or a
  widely-used open-source DFIR/inventory tool (osquery, Velociraptor,
  Hindsight).
- **[inferred]** — follows a documented convention (Flatpak
  `~/.var/app/<id>/config/…`, Snap user-data layouts) but was not
  confirmed on this machine and is not in the primary source's table.

Linux and Windows cells were not exercised here. Treat them as
[documented] or [inferred] until slice 5's CI matrix proves them.

---

## 1. Shared layout (all browsers in this table)

Chromium stores per-install state in a **user-data directory**. Each
profile is a subdirectory of that directory. Session restore files live
inside the profile, not at the user-data root.

```
<user-data-dir>/
  Local State                          # JSON, per-install (profile list)
  Default/                             # first profile (chrome::kInitialProfile)
  Profile 1/                           # chrome::kMultiProfileDirPrefix + N
  Guest Profile/                       # skip — not a real browsing session
  System Profile/                      # skip — no browser windows
  <profile>/
    Preferences                        # JSON, per-profile
    Sessions/                          # sessions::kSessionsDirectory (Chrome 85+)
      Session_<filetime>               # open windows/tabs  (SessionService)
      Tabs_<filetime>                  # recently-closed    (TabRestoreService)
      Apps_<filetime>                  # PWA windows        (AppSessionService)
    Sessions_Encrypted/                # Chrome 148+; see §1.3
```

**The path from user-data dir to Sessions is always:**

```
<user-data-dir>/<profile-dir>/Sessions/
```

This relative path does not vary by OS or by Chromium fork. What varies
is only `<user-data-dir>` (and, for Arc, an extra `User Data/` component
on macOS — see §2).

Confirm a running instance with `chrome://version` (or `brave://version`,
`edge://version`, `vivaldi://version`, `opera://version`): **Profile
Path** is `<user-data-dir>/<profile-dir>`; the user-data dir is its
parent.

### 1.1 Which file is "what's open"

| File prefix | Chromium service | Meaning | Use for `save`? |
|---|---|---|---|
| `Session_*` | SessionService | Windows and tabs that restore on startup | **Yes — this is the snapshot** |
| `Tabs_*` | TabRestoreService | Recently-closed tabs | No (not currently open) |
| `Apps_*` | AppSessionService | Installed web-app windows | Not for slice 4 |

The reference implementation already selects `Session_*` whose suffix is
all digits, then takes `max(int(suffix))`. Keep that rule. Chrome's 2020
rewrite ([crrev.com/c/2238885](https://chromium-review.googlesource.com/c/chromium/src/+/2238885),
Chrome 85) made the numeric suffix the recency key, and forces each new
file's timestamp to be greater than any existing one so clock changes
cannot rewind restore.

Do not confuse `Sessions/` with `Session Storage/` (the HTML5
`sessionStorage` LevelDB). Different directory, different format.

### 1.2 Legacy names (pre-Chrome 85)

If `Sessions/` is missing or empty, Chromium falls back to these files
in the **profile directory itself**:

- `Current Session`, `Last Session`
- `Current Tabs`, `Last Tabs`

They are still SNSS. Probe them only as a fallback. They are gone on
every Chromium-family profile checked on this machine.

### 1.3 `Sessions_Encrypted/` (Chrome 148+)

Chromium added `sessions::kEncryptedSessionsDirectory = "Sessions_Encrypted"`
in Chrome 148 ([session_constants.h](https://sources.debian.org/src/chromium/149.0.7827.114-1/components/sessions/core/session_constants.h/)).
On this machine Chrome's Default profile has both directories; the
encrypted files still begin with the four-byte `SNSS` magic, but the
payload format is not the cleartext command log. **Ignore
`Sessions_Encrypted/` until a later slice.** Prefer cleartext
`Sessions/Session_*`. If a future Chrome build stops writing cleartext
files, that is a parser change, not a path-table change.

---

## 2. The path matrix — user-data directory

One row per browser, one column per OS. Cells are the **user-data
directory**. Join `"/<profile>/Sessions"` on every OS.

Linux cells list the **native** path first. Snap and Flatpak are in
§2.1 because they are different directories, not aliases.

Windows `%LOCALAPPDATA%` is the known-folder `FOLDERID_LocalAppData`
(typically `C:\Users\<user>\AppData\Local`). Windows `%APPDATA%` is
`FOLDERID_RoamingAppData` (`…\AppData\Roaming`). Opera is the only
browser in this table that uses roaming.

### 2.0 Stable / primary channels

| Browser | macOS | Linux (native) | Windows | Tag |
|---|---|---|---|---|
| **Chrome** | `~/Library/Application Support/Google/Chrome` | `~/.config/google-chrome` | `%LOCALAPPDATA%\Google\Chrome\User Data` | macOS **[verified-on-this-machine]**; Linux/Windows **[documented]** ([Chromium `user_data_dir.md`](https://chromium.googlesource.com/chromium/src/+/HEAD/docs/user_data_dir.md), [osquery `utils.cpp`](https://github.com/osquery/osquery/blob/master/osquery/tables/applications/chrome/utils.cpp), [Hindsight](https://github.com/RyanDFIR/hindsight)) |
| **Chrome Beta** | `~/Library/Application Support/Google/Chrome Beta` | `~/.config/google-chrome-beta` | `%LOCALAPPDATA%\Google\Chrome Beta\User Data` | **[documented]** (same three sources) |
| **Chrome Dev** | `~/Library/Application Support/Google/Chrome Dev` | `~/.config/google-chrome-unstable` | `%LOCALAPPDATA%\Google\Chrome Dev\User Data` | **[documented]** (Chromium `user_data_dir.md`; Linux suffix is `-unstable`, not `-dev`) |
| **Chrome Canary** | `~/Library/Application Support/Google/Chrome Canary` | `~/.config/google-chrome-canary` | `%LOCALAPPDATA%\Google\Chrome SxS\User Data` | **[documented]** (Windows directory is `Chrome SxS`, not `Chrome Canary`) |
| **Chromium** | `~/Library/Application Support/Chromium` | `~/.config/chromium` | `%LOCALAPPDATA%\Chromium\User Data` | macOS dir exists here but is leftover (no `Local State`) **[verified-on-this-machine]** as a path; contents empty. Linux/Windows **[documented]**. osquery's Windows Chromium suffix omits `\User Data` — that is an osquery bug; Chromium source and `user_data_dir.md` include it. |
| **Brave** | `~/Library/Application Support/BraveSoftware/Brave-Browser` | `~/.config/BraveSoftware/Brave-Browser` | `%LOCALAPPDATA%\BraveSoftware\Brave-Browser\User Data` | macOS **[verified-on-this-machine]**; Linux/Windows **[documented]** ([Brave Community](https://community.brave.app/t/where-brave-browser-profiles-are-located/646708), osquery). **Do not trust** Brave's own "delete my data" help article: it writes `Brave Software` (space) and `~/.config/Brave-browser`. Those paths are wrong. Brave's crash-report article uses the correct Linux path. |
| **Edge** | `~/Library/Application Support/Microsoft Edge` | `~/.config/microsoft-edge` | `%LOCALAPPDATA%\Microsoft\Edge\User Data` | macOS **[verified-on-this-machine]**; Linux/Windows **[documented]** ([Microsoft Learn `UserDataDir`](https://learn.microsoft.com/en-us/deployedge/microsoft-edge-policies/userdatadir), osquery, [Foxton](https://www.foxtonforensics.com/browser-history-examiner/microsoft-edge-history-location)) |
| **Vivaldi** | `~/Library/Application Support/Vivaldi` | `~/.config/vivaldi` | `%LOCALAPPDATA%\Vivaldi\User Data` | macOS **[verified-on-this-machine]**; Linux/Windows **[documented]** ([Vivaldi Help](https://help.vivaldi.com/zh-hans/desktop-zh-hans/privacy-zh-hans/preventing-vivaldi-profiles-from-being-uploaded-to-git-repositories/), osquery) |
| **Opera** | `~/Library/Application Support/com.operasoftware.Opera` | `~/.config/opera` | `%APPDATA%\Opera Software\Opera Stable` | macOS path exists here as a leftover (`NativeMessagingHosts` only, no `Local State`) **[verified-on-this-machine]** as a path; Linux/Windows **[documented]** (osquery; [Opera profile-layout wiki](https://www.reddit.com/r/operabrowser/wiki/opera/new_profile_layout); Velociraptor `Opera Software\Opera*Stable`) |
| **Arc** | `~/Library/Application Support/Arc/User Data` | *(no desktop Linux product)* | Store: `%LOCALAPPDATA%\Packages\TheBrowserCompany.Arc_ttt1ap7aakyb4\LocalCache\Local\Arc\` · unpackaged: `%LOCALAPPDATA%\Arc\User Data` | macOS **[verified-on-this-machine]** (osquery lists the same `Arc/User Data`). Windows **[documented]** by exporters, untested here. **Exclude — see §3.** |

CLI ids for `--browser`: `chrome`, `chrome-beta`, `chrome-dev`,
`chrome-canary`, `chromium`, `brave`, `edge`, `vivaldi`, `opera`.
Channel variants of Brave and Edge are in §2.2; they are not required
for slice 4 but the paths are cheap to keep in the table.

### 2.1 Linux packaging variants (probe in this order per browser)

Native, Snap, and Flatpak installs of the *same* browser do **not**
share a user-data dir. A machine can have two. Auto-detect must probe
all of them and let recency pick (see §6).

Host `$XDG_CONFIG_HOME` applies to **native** paths only. Snap and
Flatpak hard-code their own config roots; do not rewrite those with the
host XDG value.

| Browser | Snap | Flatpak | Tag |
|---|---|---|---|
| Chrome | *(Google does not ship an official Chrome snap)* | `~/.var/app/com.google.Chrome/config/google-chrome` | Flatpak **[inferred]** from Flathub id `com.google.Chrome` + XDG-in-sandbox |
| Chromium | `~/snap/chromium/common/chromium` (current) · `~/snap/chromium/current/.config/chromium` (legacy, pre-migration) | `~/.var/app/org.chromium.Chromium/config/chromium` | Snap current + Flatpak **[documented]** (osquery). Legacy snap **[documented]** ([Ask Ubuntu](https://askubuntu.com/questions/1455357/how-to-get-consistent-default-user-data-dir-when-installing-snap-chromium)) |
| Brave | `~/snap/brave/common/.config/BraveSoftware/Brave-Browser` · `~/snap/brave/current/.config/BraveSoftware/Brave-Browser` | `~/.var/app/com.brave.Browser/config/BraveSoftware/Brave-Browser` | Flatpak **[documented]** (osquery). Snap **[documented]** ([GoLinuxCloud](https://www.golinuxcloud.com/backup-brave-browser-session-ubuntu/), Brave Community) |
| Brave Beta / Nightly | *(uncommon)* | `~/.var/app/com.brave.Browser/config/BraveSoftware/Brave-Browser-Beta` (and `-Nightly`) | **[documented]** (osquery) |
| Edge | *(no official snap)* | `~/.var/app/com.microsoft.Edge/config/microsoft-edge` | **[inferred]** from Flathub id `com.microsoft.Edge` |
| Vivaldi | *(uncommon)* | `~/.var/app/com.vivaldi.Vivaldi/config/vivaldi` | **[documented]** (osquery) |
| Opera | *(uncommon)* | `~/.var/app/com.opera.Opera/config/opera` | **[inferred]** from Flathub id `com.opera.Opera` |

On Linux, also honour `$CHROME_CONFIG_HOME` and `$CHROME_USER_DATA_DIR`
for Chrome/Chromium only — see §7.

### 2.2 Other channels (keep in the table, do not auto-prefer)

| Browser | macOS | Linux | Windows |
|---|---|---|---|
| Brave Beta | `~/Library/Application Support/BraveSoftware/Brave-Browser-Beta` | `~/.config/BraveSoftware/Brave-Browser-Beta` | `%LOCALAPPDATA%\BraveSoftware\Brave-Browser-Beta\User Data` |
| Brave Nightly | `~/Library/Application Support/BraveSoftware/Brave-Browser-Nightly` | `~/.config/BraveSoftware/Brave-Browser-Nightly` | `%LOCALAPPDATA%\BraveSoftware\Brave-Browser-Nightly\User Data` |
| Edge Beta | `~/Library/Application Support/Microsoft Edge Beta` | `~/.config/microsoft-edge-beta` | `%LOCALAPPDATA%\Microsoft\Edge Beta\User Data` |
| Edge Dev | `~/Library/Application Support/Microsoft Edge Dev` | `~/.config/microsoft-edge-dev` | `%LOCALAPPDATA%\Microsoft\Edge Dev\User Data` |
| Edge Canary | `~/Library/Application Support/Microsoft Edge Canary` | *(not shipped)* | `%LOCALAPPDATA%\Microsoft\Edge SxS\User Data` |
| Vivaldi Snapshot | `~/Library/Application Support/Vivaldi Snapshot` | `~/.config/vivaldi-snapshot` | `%LOCALAPPDATA%\Vivaldi Snapshot\User Data` |
| Opera GX | `~/Library/Application Support/com.operasoftware.OperaGX` | — | `%APPDATA%\Opera Software\Opera GX Stable` |
| Opera Beta / Developer | `…/com.operasoftware.OperaBeta` · `…/com.operasoftware.OperaDeveloper` | `~/.config/opera-beta` · `~/.config/opera-developer` | `%APPDATA%\Opera Software\Opera Next` / `Opera Developer` |

All of these are **[documented]** (osquery for Brave/Edge/Vivaldi
channels; Vivaldi snapshot paths from [Vivaldi blog](https://vivaldi.com/blog/desktop/thursday-evening-testing-vivaldi-browser-snapshot-3206-28/);
Opera GX from the Opera profile-layout wiki). None were installed on
this machine.

`--browser` should accept the stable id only in slice 4 (`brave`, not
`brave-beta`) unless a channel id is passed explicitly. Auto-detect
**does** scan channels: a developer whose newest session is Canary
should get Canary.

### 2.3 Opera profile-root quirk

Modern Opera (Chromium-based, ~2018+) uses the same `Default/`
subdirectory as Chrome: `<user-data>/Default/Sessions/`. Older Opera
and some DFIR cheat-sheets put `Sessions/` directly under
`Opera Stable`. Probe in this order:

1. `<user-data>/Default/Sessions/Session_*`
2. `<user-data>/Sessions/Session_*` (treat `<user-data>` as the profile)

osquery already special-cases Opera as "the user-data dir might itself
be a profile" (it looks for `Preferences` at the root before descending).

### 2.4 What the Windows "User Data" component is

On Windows, Chromium appends `chrome::kUserDataDirname` (`"User Data"`)
under the product directory ([`chrome_constants.h`](https://chromium.googlesource.com/chromium/src/+/main/chrome/common/chrome_constants.h),
[`chrome_paths_win.cc`](https://source.chromium.org/chromium/chromium/src/+/main:chrome/common/chrome_paths_win.cc)).
On macOS and Linux the product directory *is* the user-data directory;
there is no extra `User Data` folder.

**Exceptions:** Arc (macOS) uses `Arc/User Data/` because the parent
`Arc/` folder also holds Arc's own JSON (`StorableSidebar.json`, …).
Windows Chrome/Edge/Brave/Vivaldi/Chromium all have `\User Data`.
Windows Opera does not — `Opera Stable` is the user-data dir.

---

## 3. Arc — exclude

Arc is Chromium-based. It does write a Chromium user-data tree:

```
~/Library/Application Support/Arc/
  StorableSidebar.json          # Arc's sidebar / spaces / pinned tabs
  StorableWindows.json
  StorableArchiveItems.json
  User Data/                    # Chromium user-data dir
    Local State
    Default/
      Sessions/
        Tabs_*                  # SNSS, TabRestoreService only
        (no Session_* files)
```

Verified on this machine:

- `Arc.app` is installed.
- `StorableSidebar.json` exists (JSON object, top-level keys
  `sidebar`, `sidebarSyncState`, `firebaseSyncState`, `version`).
- `User Data/Default/Sessions/` exists, is not a symlink, and contains
  `Tabs_*` files whose first four bytes are `SNSS`. **`Session_*` count
  is zero.**

That is the whole story for knowmoretabs:

- The files we parse (`Session_*`) are the Chromium SessionService
  log of open windows. Arc does not write them.
- The files Arc *does* write in `Sessions/` (`Tabs_*`) are the
  recently-closed list, which is not "what's open".
- What an Arc user thinks of as open tabs — Spaces, pinned sidebar
  items, the unpinned "Today" list — lives in `StorableSidebar.json`,
  a proprietary JSON schema with no SNSS commands, no Chromium
  navigation pickle, and no compatibility with the slice-1 parser.

Exporters in the wild (`arcaeologist`, `arc-bookmarks`, SupaSidebar,
tabgroupvault) all read `StorableSidebar.json`, not SNSS. osquery lists
`Arc/User Data` as a Chrome-like profile root, which is how we found
the Chromium tree; that does not make the session files useful.

**Recommendation: exclude Arc from `--browser`, from auto-detect, and
from the slice-4 support matrix.** Half-supporting it (parsing `Tabs_*`
or pretending `User Data/Default` is Chrome) would snapshot the wrong
model and teach users the tool "works with Arc" when it does not.

Dia (The Browser Company's successor) *does* write `Session_*` SNSS on
this machine, but it is out of scope for this table. Do not silently
add it.

Arc for Windows was a Store app and is discontinued; the two Windows
paths in §2.0 are recorded so a later "we found leftover data" error
can name them, not so we parse them.

---

## 4. Profile discovery (`Local State`)

File: `<user-data-dir>/Local State` (filename
`chrome::kLocalStateFilename`). JSON. Per-install, not per-profile.

Pref names from Chromium
[`chrome/common/pref_names.h`](https://source.chromium.org/chromium/chromium/src/+/main:chrome/common/pref_names.h):

| JSON path | C++ pref | Meaning |
|---|---|---|
| `profile.last_used` | `prefs::kProfileLastUsed` | Directory basename of the last-used profile (`"Default"`, `"Profile 1"`, …). **Not the display name.** |
| `profile.last_active_profiles` | `prefs::kProfilesLastActive` | List of directory basenames that currently have a browser window (used for session restore across profiles). |
| `profile.info_cache` | `prefs::kProfileAttributes` | Map of directory basename → attributes. This is the profile list. |
| `profile.profiles_order` | `prefs::kProfilesOrder` | Directory basenames in picker order. |
| `profile.profiles_created` | `prefs::kProfilesNumCreated` | Counter used to name `Profile N`. Do not use for discovery. |

`info_cache` entry keys from
[`profile_attributes_entry.cc`](https://chromium.googlesource.com/chromium/src/+/main/chrome/browser/profiles/profile_attributes_entry.cc)
(the ones we care about):

| Key | Use |
|---|---|
| `name` | Human-readable local profile name. **This is "Work".** |
| `is_using_default_name` | `true` → Chrome is showing "Person N"; `false` → the user set `name`. |
| `gaia_name` / `gaia_given_name` | Signed-in Google account name. Do not use as `--profile`. |
| `user_name` | Account email. Do not use as `--profile`. |
| `shortcut_name` | Windows shortcut label. Often absent (it was missing on every browser on this machine). |
| `active_time` | Last-active, seconds since Unix epoch (double). Fine as a recency hint; the `Session_*` suffix is better. |
| `is_ephemeral` | Skip if true. |
| `gaia_id` | Present when signed in. Do not store in snapshot metadata. |

Directory basenames Chromium itself creates
([`chrome_constants.h`](https://chromium.googlesource.com/chromium/src/+/main/chrome/common/chrome_constants.h)):

- `Default` — `kInitialProfile`
- `Profile N` — `kMultiProfileDirPrefix` + `profiles_created`
- `Guest Profile` — skip
- `System Profile` — skip (verified here: directory exists, no `Sessions/`)

Users cannot pick an arbitrary directory name in the UI. They pick a
**display name**, which is stored in `info_cache.<dir>.name`. The
directory stays `Default` / `Profile 1`.

### 4.1 `--profile` resolution

Accept either a directory name or a display name.

```
input
  │
  ├─ exact match against an info_cache key          → that directory
  ├─ exact match against info_cache.*.name          → that directory
  │     (if two entries share the name, error:
  │      "name is used by Profile 1 and Profile 4; pass the directory")
  ├─ on macOS/Windows only: case-insensitive retry
  └─ else error, listing dir (name) pairs
```

Then run the path guard (§5) on the **directory name**, never on the
display name. Display names may contain `/`, `\`, `..`, non-ASCII; they
must never be joined onto a filesystem path.

`--profile Default` continues to work. `--profile Work` works once
`info_cache` has `"name": "Work"` on some `Profile N`.

If `--profile` is omitted:

1. `profile.last_used` if it is a non-empty string and the directory
   exists and is not Guest/System.
2. Else `profile.last_active_profiles[0]` under the same checks.
3. Else `Default` if that directory exists.
4. Else the first `info_cache` key that has a `Sessions/Session_*` file.

**`last_used` is not always present.** On this machine Chrome had it;
Brave, Edge, Vivaldi, and Arc did not. They did have
`last_active_profiles` and `info_cache`. The reference implementation
only reads `last_used` and falls back to the literal `"Default"` — that
is the bug to fix.

### 4.2 Redacted `Local State` shape

Invented values. Field *names* match Chromium and the files on this
machine; field *values* do not.

```json
{
  "profile": {
    "info_cache": {
      "Default": {
        "active_time": 1726800000.0,
        "avatar_icon": "chrome://theme/IDR_PROFILE_AVATAR_26",
        "gaia_id": "",
        "gaia_name": "",
        "is_consented_primary_account": false,
        "is_ephemeral": false,
        "is_using_default_avatar": true,
        "is_using_default_name": true,
        "name": "Person 1",
        "user_name": ""
      },
      "Profile 1": {
        "active_time": 1726801111.0,
        "gaia_given_name": "Alex",
        "gaia_id": "1234567890",
        "gaia_name": "Alex Example",
        "is_using_default_name": false,
        "name": "Work",
        "user_name": "alex@example.com"
      }
    },
    "last_active_profiles": ["Profile 1"],
    "last_used": "Profile 1",
    "profiles_order": ["Default", "Profile 1"]
  }
}
```

Snapshot metadata should keep both:

- `profile`: directory basename (`Profile 1`) — round-trips `--profile`
- `profile_display`: `info_cache` `name` (`Work`) — what the UI shows

The brief's `Source.profile` can stay the directory name; add
`profile_display` rather than overloading it.

Per-profile `Preferences` also has `profile.name`. Use `Local State`
so we do not open every profile's Preferences just to list them.

---

## 5. Safety rules

The `--profile` value is joined onto a filesystem path. The reference
implementation's guard is:

```python
if (not isinstance(profile, str)
    or not profile
    or Path(profile).name != profile
    or profile in ('.', '..')):
    raise ValueError('Profile must be a Chrome profile directory name, such as Default')
```

(`reference/b_tabs_export.py`, `choose_session`.)

`Path(p).name != p` rejects anything with a path separator, a trailing
slash, `.`, `..`, and absolute paths: `foo/bar`, `foo\bar`, `Default/`,
`/etc`, `C:\Windows`. That is the traversal attack, and the guard
stops it. After the join, `user_data / profile` cannot walk out via
`..` because `profile` cannot contain `..`.

### 5.1 What the guard already does well

- No directory separators, so no `../../`.
- Empty string rejected.
- `.` and `..` rejected.
- Works for the names Chromium actually creates (`Default`, `Profile 1`
  with a space).

### 5.2 What it misses (fix in Rust)

1. **It never resolves display names.** `--profile Work` looks for a
   folder literally named `Work`, which Chromium will not have created.
   Resolve via `info_cache` *first*, then guard the directory name.
2. **`last_used` must go through the same guard.** Chrome writes a
   basename, but we should not trust a corrupted `Local State`.
3. **Guest Profile / System Profile** pass the guard and then fail
   confusingly (no `Session_*`). Reject them by name with an explicit
   error.
4. **Symlink after join.** If `<user-data>/Default` is a symlink
   pointing outside the user-data dir, `Path.name` still equals
   `Default`. For a read-only personal tool, *following* that symlink
   is usually what the user wanted (they moved the profile). Do follow
   it for reading. After `canonicalize`, require the target is a
   directory and that we only glob `Session_*` regular files. Do not
   write anywhere under the browser tree (the brief already forbids
   this).
5. **Windows reserved device names** (`CON`, `PRN`, `AUX`, `NUL`,
   `COM1`, `LPT1`, …). The guard allows them; `CreateFile` then does
   something surprising. Reject with a allowlist: `Default`,
   `Profile <digits>`, or a key that actually appears in `info_cache`.
   An allowlist-from-`info_cache` is stronger than the reference guard
   and still accepts every real profile.
6. **Windows trailing dots/spaces** (`Default.`). Reject; Chromium
   will not have created them.
7. **NTFS alternate data streams** (`Default:secret`). `Path.name`
   equals the whole string on some Python/Windows combinations. The
   `info_cache` allowlist stops this.
8. **Null bytes.** Rust `Path` cannot represent a NUL on Unix; still
   reject any `OsStr` containing `0u8` before joining.

Recommended implementation: after display-name resolution, require
`profile` is a single path component **and** (`info_cache` has that key
**or** it matches `^Default$|^Profile [0-9]+$`). Then
`user_data.join(profile)` and, if `canonicalize` succeeds, glob
`Session_*` with a digit suffix, files only.

Do **not** refuse the user-data dir itself being a symlink. Moving
`~/Library/Application Support/Google/Chrome` and replacing it with a
symlink is a documented way to relocate a profile.

### 5.3 Non-ASCII

Profile **directory** names Chromium generates are ASCII. Profile
**display** names can be any Unicode (emoji, CJK, combining marks).
Never put a display name in a path. When matching `--profile` against
`name`, compare Unicode scalar values as stored in the JSON (UTF-8);
do not NFKC-normalise unless a real user hits a mismatch.

Rust `std::fs` on Windows already uses WTF-16, so a Unicode *directory*
(if a user created one via `--user-data-dir` and a hand-made folder)
works. No extra encoding layer.

### 5.4 Windows path length

`chrome_paths_win.cc` still talks to `SHGetFolderPath` with `MAX_PATH`
(260). A realistic session path:

```
%LOCALAPPDATA%\Google\Chrome\User Data\Profile 1\Sessions\Session_13233899802170163
```

is under 260 for normal usernames. A custom `--user-data-dir` plus a
deep prefix is not. Enable `longPathAware` in the Windows manifest and
use `\\?\` only if a create/open fails with `ERROR_FILENAME_EXCED_RANGE`.
Do not pre-emptively prefix; it changes how `..` and `/` are parsed.

### 5.5 Symlinks we saw

On this machine none of the user-data dirs, and none of the
`Default/Sessions` dirs we checked (Chrome, Brave, Edge, Vivaldi, Arc),
were symlinks.

---

## 6. Detection strategy (zero flags)

**Pick the newest `Session_*` across every installed supported
browser. Do not require `--browser`. Do not prefer Chrome just because
the reference implementation did.**

Slice 4 exists to capture "the browser you actually use". A static
priority list (Chrome, then Chromium, then Brave, …) snapshots the
wrong browser the moment someone lives in Edge. Requiring a flag
breaks the 80/20: `knowmoretabs save` has to work as one command.

Algorithm:

1. Build the candidate list from §2 for the current OS, including
   channel and Linux packaging variants, **excluding Arc**.
2. Apply env overrides from §7 (this may add or replace a Chrome
   user-data dir).
3. A candidate counts as installed if its user-data dir exists **and**
   it has at least one non-guest profile with a `Session_*` file
   (digit suffix, regular file). Leftover empty dirs (Chromium and
   Opera on this machine) do not count.
4. For each candidate, resolve the profile as in §4.1 (no `--profile`
   yet). Record `(browser_id, profile_dir, profile_display, session_path,
   recency)` where recency is the numeric suffix of `Session_*` (the
   same key Chrome uses). Fall back to `mtime` only if the suffix is
   unparseable.
5. If `--browser` / `--profile` / `--session` were passed, filter
   first; error if the filter matches nothing, naming the path we
   looked at.
6. If the list is empty: error, tell the user to pass `--browser` or
   `--session`, and mention `chrome://version` → Profile Path.
7. If the list has one entry: use it.
8. If the list has several: use the maximum recency. Always print
   which one won. If others exist, name them and how to override.

Within one browser, do **not** pick the profile with the newest
session. `last_used` / `last_active_profiles` is the profile whose
windows are on screen; another profile may have a newer file from a
background sync or an unused window. Cross-browser recency is the
right signal; cross-profile recency inside one browser is not.

Tie-break when suffixes are equal: stable channel before beta/dev/canary,
then the `--browser` id lexicographically. This should almost never
happen (Chrome forces monotonic suffixes *within* a profile, not
across browsers).

### 6.1 What `knowmoretabs save` prints when it finds three browsers

Human (default):

```
saved 247 tabs from chrome / Default (Person 1)
  also found:
    brave  / Default (Work)       12 minutes older  — --browser brave
    edge   / Default (Person 1)    2 days older     — --browser edge
```

Rules for that block:

- The first line always names `browser / dir (display)`. Always
  include the display name, even when it is the default "Person 1".
- "also found" is one line per other browser, recency as a relative
  age against the winner, plus the exact flag to override. Do not
  hide this behind `-v`. A user who lives in Brave and just opened
  Chrome to check one thing needs to see the override.
- Do not print full filesystem paths in the default summary; `-v`
  adds `source: <session path>`.
- Do not prompt. Save the newest. Say what you did.

`--json` (same situation):

```json
{
  "saved": {
    "browser": "chrome",
    "profile": "Default",
    "profile_display": "Person 1",
    "tabs": 247,
    "snapshot": "2026-09-20-084415Z"
  },
  "also_found": [
    {"browser": "brave", "profile": "Default", "profile_display": "Work", "age_seconds": 720},
    {"browser": "edge",  "profile": "Default", "profile_display": "Person 1", "age_seconds": 172800}
  ]
}
```

If the user passes `--browser brave`, skip the scan of the others
(still fine to mention "chrome also has a session, 12 minutes newer"
under `-v` only — not in the default line, they opted in).

---

## 7. Environment overrides

| Variable / flag | Who honours it | What we should do |
|---|---|---|
| `--user-data-dir=` (Chrome launch flag) | Chrome on every desktop OS ([`chrome_main_delegate.cc`](https://source.chromium.org/chromium/chromium/src/+/main:chrome/app/chrome_main_delegate.cc), `user_data_dir.md`) | **Do not scrape running processes** to find this. Expose the same flag on `knowmoretabs` as an escape hatch (the brief already has `--session FILE`; `--user-data-dir DIR` is the next-less-raw version). If passed, that directory *is* the user-data dir for whichever `--browser` is set, defaulting to `chrome`. |
| `CHROME_USER_DATA_DIR` | **Linux (and ChromeOS) only.** Read by `chrome_main_delegate.cc` when `--user-data-dir` is absent. `--user-data-dir` wins if both are set. | Honour on Linux, and only as a candidate for `chrome` and `chromium`. Do **not** apply it to Brave/Edge/Vivaldi/Opera — even though they are Chromium forks, a user who set this for Chrome Remote Desktop would otherwise have every browser "found" at the same path. |
| `CHROME_CONFIG_HOME` | Linux, Chrome/Chromium, since M61. Replaces `~/.config` so channels still get distinct suffixes (`google-chrome`, `google-chrome-beta`, …). | Honour on Linux for `chrome` / `chromium` default-path computation. Native only. |
| `XDG_CONFIG_HOME` | Linux, any XDG app. Chromium uses it when `CHROME_CONFIG_HOME` is unset (`chrome_paths_linux.cc`). | Honour on Linux for **native** paths of every browser in the table (`$XDG_CONFIG_HOME/google-chrome`, `$XDG_CONFIG_HOME/BraveSoftware/Brave-Browser`, …). Do not apply to Snap/Flatpak paths. |
| `%LOCALAPPDATA%` | Windows. Chromium goes through `PathService(DIR_LOCAL_APP_DATA)`, which honours this. | Honour via the known-folder API (`FOLDERID_LocalAppData`), not by concatenating the env var ourselves. The API is what Chrome uses and it still works if the variable is unset. |
| `%APPDATA%` | Windows roaming. Opera only, in this table. | Same: `FOLDERID_RoamingAppData`. |
| `UserDataDir` policy | Chrome/Edge enterprise policy, overrides even `--user-data-dir`. | Out of scope for slice 4. If a user has a policy-relocated profile, they pass `--user-data-dir` or `--session`. Mention `chrome://version` in the "not found" error. |

Priority when several apply to Chrome on Linux:

1. knowmoretabs `--user-data-dir` / `--session`
2. `CHROME_USER_DATA_DIR` (full replacement of that browser's dir)
3. `CHROME_CONFIG_HOME` / `<product>`
4. `XDG_CONFIG_HOME` / `<product>`
5. `~/.config/<product>`
6. Snap / Flatpak paths as extra candidates, not replacements

`--user-data-dir` on the *browser* is invisible to us unless the user
repeats it. That is acceptable. Document it in the README next to
`--session`.

---

## 8. Verification on this machine (macOS, 2026-09-20)

Read-only: `test -d`, directory listings, filename-prefix counts, the
four-byte magic of `Session_*` / `Tabs_*`, and `Local State` **key
names** (no values). No session payloads, no display names, no URLs.

### 8.1 App bundles present

`/Applications` contained: `Google Chrome.app`, `Brave Browser.app`,
`Microsoft Edge.app`, `Vivaldi.app`, `Arc.app`. (Also `Dia.app`, out
of scope.) No Opera, no Chromium, no Chrome Beta/Dev/Canary.

### 8.2 User-data dirs

| Browser | User-data dir | Local State | Default/Sessions | `Session_*` | `Tabs_*` | Magic | Tag |
|---|---|---|---|---|---|---|---|
| Chrome | yes | yes | yes | 3 (Default) | 3 | `SNSS` | **[verified-on-this-machine]** |
| Chrome Beta / Dev / Canary | no | — | — | — | — | — | **[documented]** only |
| Chromium | leftover (`Crashpad`, `NativeMessagingHosts`) | no | no | 0 | 0 | — | path **[verified]** empty |
| Brave | yes | yes | yes | 2 | 2 | `SNSS` | **[verified-on-this-machine]** |
| Edge | yes | yes | yes | 2 | 2 | `SNSS` | **[verified-on-this-machine]** |
| Vivaldi | yes | yes | yes | 2 | 2 | `SNSS` | **[verified-on-this-machine]** |
| Opera | leftover (`NativeMessagingHosts`) | no | no | 0 | 0 | — | path **[verified]** empty |
| Arc `Arc/` | yes | n/a (not the user-data dir) | n/a | — | — | — | `StorableSidebar.json` present |
| Arc `Arc/User Data` | yes | yes | yes | **0** | 2 | `Tabs_*` is `SNSS` | **[verified-on-this-machine]** |

Chrome also had `Default/Sessions_Encrypted/` (4 files, `Session_*` and
`Tabs_*` prefixes, first four bytes `SNSS`). Other browsers did not.
None of the user-data or `Sessions` directories were symlinks.

Chrome had five `Default` / `Profile N` directories plus `System
Profile` (no `Sessions/`). Brave, Edge, Vivaldi, Arc each had a single
`Default`. All `info_cache` keys on this machine were defaultish
directory names (`Default` / `Profile N`); no custom directory names.

`profile.last_used` was present on Chrome and absent on Brave, Edge,
Vivaldi, and Arc. All five had `profile.info_cache`,
`profile.last_active_profiles`, and `profile.profiles_order`.
`info_cache.*.name` and `is_using_default_name` were present on all
five. `shortcut_name` was present on none.

### 8.3 Count for the report

From the requested list: **5 browsers have an app bundle** (Chrome,
Brave, Edge, Vivaldi, Arc). **4 of those write `Session_*` SNSS**
(Chrome, Brave, Edge, Vivaldi). Arc is installed and has a Chromium
user-data tree, but no `Session_*` files.

---

## 9. Implementation notes for slices 4 and 5

A static table is enough. Suggested shape:

```text
BrowserSpec {
  id:            "chrome" | "chrome-beta" | …   # --browser value
  user_data:     per-OS path templates
  linux_extra:   snap/flatpak paths to probe
  windows_root:  LocalAppData | RoamingAppData
  include_in_autodetect: bool   # false for Arc
}
```

Sessions path = `user_data / profile / "Sessions" / "Session_*"`.
No per-browser special cases except:

- Opera's optional profile-at-root fallback (§2.3)
- Arc: not in the autodetect set
- Chrome 148 `Sessions_Encrypted/`: ignored

The reference implementation's macOS-Chrome-only hardcode
(`Path.home() / "Library/Application Support/Google/Chrome"`) is the
thing this table replaces.

---

## 10. Sources

### Official / source of truth

- [Chromium `docs/user_data_dir.md`](https://chromium.googlesource.com/chromium/src/+/HEAD/docs/user_data_dir.md)
  — default user-data paths per OS and channel; `--user-data-dir`;
  Linux `CHROME_USER_DATA_DIR`, `CHROME_CONFIG_HOME`, `XDG_CONFIG_HOME`.
- [`chrome/common/chrome_paths_linux.cc`](https://chromium.googlesource.com/chromium/src/+/HEAD/chrome/common/chrome_paths_linux.cc)
  — `GetDefaultUserDataDirectory`; `CHROME_CONFIG_HOME` then XDG.
- [`chrome/common/chrome_paths_win.cc`](https://source.chromium.org/chromium/chromium/src/+/main:chrome/common/chrome_paths_win.cc)
  — `DIR_LOCAL_APP_DATA` + `kUserDataDirname`.
- [`chrome/app/chrome_main_delegate.cc`](https://source.chromium.org/chromium/chromium/src/+/main:chrome/app/chrome_main_delegate.cc)
  — `CHROME_USER_DATA_DIR` is Linux/ChromeOS only.
- [`chrome/common/chrome_constants.h`](https://chromium.googlesource.com/chromium/src/+/main/chrome/common/chrome_constants.h)
  — `kInitialProfile = "Default"`, `kMultiProfileDirPrefix = "Profile "`,
  `kGuestProfileDir`, `kSystemProfileDir`, `kLocalStateFilename`,
  `kUserDataDirname = "User Data"` (Windows).
- [`chrome/common/pref_names.h`](https://source.chromium.org/chromium/chromium/src/+/main:chrome/common/pref_names.h)
  — `profile.last_used`, `profile.last_active_profiles`,
  `profile.info_cache`, `profile.profiles_order`.
- [`chrome/browser/profiles/profile_attributes_entry.cc`](https://chromium.googlesource.com/chromium/src/+/main/chrome/browser/profiles/profile_attributes_entry.cc)
  — `info_cache` field names (`name`, `gaia_name`, …).
- [`components/sessions/core/session_constants.cc`](https://chromium.googlesource.com/chromium/src/+/main/components/sessions/core/session_constants.cc)
  — `Sessions`, `Sessions_Encrypted`, prefixes `Session` / `Tabs` / `Apps`.
- [Chrome 85 session-file rewrite](https://chromium-review.googlesource.com/c/chromium/src/+/2238885)
  — `Sessions/Session_<timestamp>` layout.
- [Microsoft Learn: Edge `UserDataDir`](https://learn.microsoft.com/en-us/deployedge/microsoft-edge-policies/userdatadir)
  and [Create user data directory variables](https://learn.microsoft.com/en-us/deployedge/edge-learnmore-create-user-directory-vars).

### DFIR / inventory (independent corroboration)

- [osquery `osquery/tables/applications/chrome/utils.cpp`](https://github.com/osquery/osquery/blob/master/osquery/tables/applications/chrome/utils.cpp)
  — the most complete public path table: Chrome/Brave/Edge/Vivaldi/Opera/Arc
  on Windows, macOS, Linux, including several Flatpak and the Chromium
  snap path.
- [Hindsight (RyanDFIR)](https://github.com/RyanDFIR/hindsight) — Chrome
  default profile paths.
- [Velociraptor `Windows.Applications.Chrome.History`](https://docs.velociraptor.app/artifact_references/pages/windows.applications.chrome.history)
  — glob covering Chrome, Edge, Brave, Vivaldi, Opera on Windows.
- [Autopsy `Chromium.java`](https://sleuthkit.org/autopsy/docs/api-docs/4.19.3/_chromium_8java_source.html)
  — parses `Local State` `profile.info_cache`.

### Vendor / community (forks)

- [Brave Community: where profiles are located](https://community.brave.app/t/where-brave-browser-profiles-are-located/646708)
  — correct `BraveSoftware/Brave-Browser` paths. Prefer this over
  [support.brave.app "delete my data"](https://support.brave.app/hc/en-us/articles/4413256282765-How-do-I-delete-my-data-in-Brave),
  which is wrong on all three OSes.
- [Vivaldi Help: profile folder](https://help.vivaldi.com/zh-hans/desktop-zh-hans/privacy-zh-hans/preventing-vivaldi-profiles-from-being-uploaded-to-git-repositories/)
  and [Vivaldi snapshot user-data paths](https://vivaldi.com/blog/desktop/thursday-evening-testing-vivaldi-browser-snapshot-3206-28/).
- [Opera new profile layout](https://www.reddit.com/r/operabrowser/wiki/opera/new_profile_layout)
  — `Default` under `Opera Stable`; macOS `com.operasoftware.Opera`.
- Arc sidebar path: [osquery](https://github.com/osquery/osquery/blob/master/osquery/tables/applications/chrome/utils.cpp),
  [arcaeologist](https://github.com/plainspace/arcaeologist),
  [SupaSidebar](https://supasidebar.com/blog/export-arc-browser-sidebar).

### Linux packaging

- [Snap data locations](https://snapcraft.io/docs/data-locations) —
  `SNAP_USER_COMMON` = `~/snap/<name>/common`.
- [Ask Ubuntu: Chromium snap user-data dir](https://askubuntu.com/questions/1455357/how-to-get-consistent-default-user-data-dir-when-installing-snap-chromium)
  — `common/chromium` vs legacy `current/.config/chromium`.
- Flathub ids: `org.chromium.Chromium`, `com.brave.Browser`,
  `com.vivaldi.Vivaldi`, `com.google.Chrome`, `com.microsoft.Edge`,
  `com.opera.Opera`. Sandbox config root is
  `~/.var/app/<id>/config/`.

### This repo

- `reference/b_tabs_export.py` `choose_session` — profile guard,
  `Local State` `profile.last_used`, `Sessions/Session_*` selection
  by numeric suffix. Inspiration, not a specification.
