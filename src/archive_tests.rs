//! Regression checks for `archive`.
//!
//! slice: capture, triage, platforms
//! why: Synthetic archive checks stay beside their module while keeping production storage and locking code easy to navigate.

use super::*;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

#[test]
fn id_format_is_utc_and_sortable() {
    let at: Timestamp = "2026-09-20T08:44:15.999Z".parse().unwrap();
    assert_eq!(format_id(at), "2026-09-20-084415Z");
    let later: Timestamp = "2026-09-20T08:44:16Z".parse().unwrap();
    assert!(format_id(at) < format_id(later));
    assert!(format_id(at) < format!("{}-2", format_id(at)));
}

/// "Private to the user" is not the same sentence on both platforms, so
/// each asserts its own.
///
/// Unix: mode `0700`, which we set.
///
/// Windows: there is no mode, and we cannot set an access-control list
/// without the Win32 security APIs, so the only claim that is ours to
/// make is **we add no access of our own** — the archive is exactly as
/// private as an ordinary directory created in the same place, and
/// `default_root` makes sure that place is under `%LOCALAPPDATA%`. That
/// is asserted by building an ordinary directory beside it and comparing
/// the two. An earlier version asserted every entry was inherited, which
/// was a fact about the parent rather than about us, and duly failed
/// under `%TEMP%`, where entries are explicit.
#[test]
fn open_creates_root_privately_and_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("nested").join("root");
    let archive = Archive::open(&root).unwrap();
    assert!(archive.snapshots_dir().is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&root).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
    #[cfg(windows)]
    {
        let ordinary = root.with_file_name("ordinary");
        fs::create_dir(&ordinary).unwrap();
        assert_eq!(
            windows_access_entries(&root),
            windows_access_entries(&ordinary),
            "creating the archive granted access an ordinary directory here would not have"
        );
        fs::remove_dir(&ordinary).unwrap();
    }
    Archive::open(&root).unwrap();
    assert_eq!(archive.snapshot_ids().unwrap(), Vec::<String>::new());
}

/// Who a directory grants what, read back through `icacls`, with the
/// directory's own name removed so that two directories can be compared.
/// Nothing here reads the localised text beside an entry; the entries are
/// only ever compared with each other, on the same machine, moments apart.
#[cfg(windows)]
fn windows_access_entries(path: &Path) -> Vec<String> {
    let out = std::process::Command::new("icacls")
        .arg(path)
        .output()
        .expect("icacls");
    assert!(out.status.success(), "icacls failed for {}", path.display());
    // icacls echoes the name it was given, verbatim, before the first
    // entry; that name is the one thing that must not take part in the
    // comparison, and it is a string we already hold.
    let text = String::from_utf8_lossy(&out.stdout).replace(&path.display().to_string(), "");
    let mut entries: Vec<String> = text
        .lines()
        .filter(|line| line.contains(":("))
        .map(|line| line.trim().to_owned())
        .collect();
    assert!(
        !entries.is_empty(),
        "icacls listed no entries for {}",
        path.display()
    );
    entries.sort();
    entries
}

#[test]
#[cfg(unix)]
fn staging_is_private_even_inside_an_existing_public_root() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let archive = Archive::open(tmp.path()).unwrap();
    for dir in [tmp.path().to_path_buf(), archive.snapshots_dir()] {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let stage = archive.stage().unwrap();
    assert_eq!(
        fs::metadata(stage.path()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let path = stage.path().to_owned();
    let unwind = std::panic::catch_unwind(move || {
        stage.write("History", b"private browsing data").unwrap();
        panic!("interrupted History read");
    });
    assert!(unwind.is_err());
    assert!(!path.exists(), "scratch data survived unwinding");
}

#[test]
fn allocate_id_suffixes_collisions() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = Archive::open(tmp.path()).unwrap();
    let at: Timestamp = "2026-09-20T08:44:15Z".parse().unwrap();
    assert_eq!(archive.allocate_id(at).unwrap(), "2026-09-20-084415Z");
    fs::create_dir(archive.snapshots_dir().join("2026-09-20-084415Z")).unwrap();
    assert_eq!(archive.allocate_id(at).unwrap(), "2026-09-20-084415Z-2");
    fs::create_dir(archive.snapshots_dir().join("2026-09-20-084415Z-2")).unwrap();
    assert_eq!(archive.allocate_id(at).unwrap(), "2026-09-20-084415Z-3");
}

#[test]
fn staging_is_hidden_until_published_and_cleaned_if_abandoned() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = Archive::open(tmp.path()).unwrap();
    let staging = archive.stage().unwrap();
    staging.write("a.txt", b"hello").unwrap();
    assert!(staging.path().starts_with(archive.snapshots_dir()));
    assert_eq!(archive.snapshot_ids().unwrap(), Vec::<String>::new());

    let abandoned = archive.stage().unwrap();
    let abandoned_path = abandoned.dir.keep();
    assert!(abandoned_path.is_dir());
    assert_eq!(clean_stale_staging(&archive.snapshots_dir()).unwrap(), 2);
    assert!(!abandoned_path.is_dir());
    assert!(!staging.path().is_dir());
    drop(staging);
}

#[test]
fn cleaning_a_directory_removes_staged_files_and_keeps_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::write(dir.join(format!("{STAGING_PREFIX}abc")), b"half written").unwrap();
    fs::create_dir(dir.join(format!("{STAGING_PREFIX}def"))).unwrap();
    fs::write(dir.join("kept.md"), b"published").unwrap();
    fs::create_dir(dir.join("kept")).unwrap();
    assert_eq!(clean_stale_staging(dir).unwrap(), 2);
    let mut left: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["kept", "kept.md"]);
    assert!(clean_stale_staging(&dir.join("missing")).is_err());
}

#[test]
fn publish_renames_once_and_refuses_to_overwrite() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = Archive::open(tmp.path()).unwrap();
    let staging = archive.stage().unwrap();
    staging.write("a.txt", b"hello").unwrap();
    // "Moved, not copied" is the same claim on both platforms, but only
    // Unix can be asked it on stable: `MetadataExt::file_index` is the
    // Windows equivalent of an inode and is still unstable there. The
    // assertions around this one hold everywhere.
    #[cfg(unix)]
    let staging_inode = std::fs::metadata(staging.path()).unwrap().ino();
    let published = archive.publish(staging, "2026-01-01-000000Z").unwrap();
    assert_eq!(fs::read(published.join("a.txt")).unwrap(), b"hello");
    #[cfg(unix)]
    assert_eq!(std::fs::metadata(&published).unwrap().ino(), staging_inode);
    assert_eq!(archive.snapshot_ids().unwrap(), vec!["2026-01-01-000000Z"]);
    assert_eq!(clean_stale_staging(&archive.snapshots_dir()).unwrap(), 0);

    let again = archive.stage().unwrap();
    again.write("a.txt", b"other").unwrap();
    let err = archive.publish(again, "2026-01-01-000000Z").unwrap_err();
    assert!(err.to_string().contains("already exists"));
    assert_eq!(fs::read(published.join("a.txt")).unwrap(), b"hello");
    assert_eq!(
        clean_stale_staging(&archive.snapshots_dir()).unwrap(),
        0,
        "failed publish cleans itself"
    );
}

/// The durability promise in the one shape where `rename` does not mean
/// the same thing on POSIX and Windows. POSIX `rename` replaces an empty
/// destination directory and takes the whole snapshot with it; Windows
/// refuses. Publication depends on neither, because it refuses first —
/// so this asserts the same outcome on all three platforms, against an
/// empty directory, a full one, and a file wearing a snapshot's name.
/// What is already sitting at the snapshot's name.
enum Occupied {
    EmptyDirectory,
    FullDirectory,
    File,
}

#[test]
fn publish_never_renames_over_anything_that_already_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = Archive::open(tmp.path()).unwrap();
    let occupied = [
        ("2026-01-01-000001Z", Occupied::EmptyDirectory),
        ("2026-01-01-000002Z", Occupied::FullDirectory),
        ("2026-01-01-000003Z", Occupied::File),
    ];
    for (id, what) in occupied {
        let destination = archive.snapshots_dir().join(id);
        match what {
            Occupied::EmptyDirectory => fs::create_dir(&destination).unwrap(),
            Occupied::FullDirectory => {
                fs::create_dir(&destination).unwrap();
                fs::write(destination.join("keep.txt"), b"mine").unwrap();
            }
            Occupied::File => fs::write(&destination, b"not a directory").unwrap(),
        }
        let before = fs::symlink_metadata(&destination).unwrap();
        let staging = archive.stage().unwrap();
        staging.write("snapshot.json", b"the new snapshot").unwrap();

        let err = archive.publish(staging, id).unwrap_err();
        assert!(err.to_string().contains("already exists"), "{id}: {err}");
        let after = fs::symlink_metadata(&destination).unwrap();
        assert_eq!(
            before.file_type().is_dir(),
            after.file_type().is_dir(),
            "{id}"
        );
        assert!(
            !destination.join("snapshot.json").exists(),
            "{id}: the staged snapshot reached the destination"
        );
        assert_eq!(
            clean_stale_staging(&archive.snapshots_dir()).unwrap(),
            0,
            "{id}"
        );
    }
    assert_eq!(
        fs::read(
            archive
                .snapshots_dir()
                .join("2026-01-01-000002Z")
                .join("keep.txt")
        )
        .unwrap(),
        b"mine"
    );
    assert_eq!(
        fs::read(archive.snapshots_dir().join("2026-01-01-000003Z")).unwrap(),
        b"not a directory"
    );
}

/// The other half of the rename answer: here the destination does exist
/// and has to be replaced.
///
/// The replacement happens while a reader holds the old content open,
/// because that is the case where Windows used to differ and no longer
/// does. Classic `MoveFileExW` replacement refuses a destination anyone
/// has open; `fs::rename`'s POSIX-semantics retry unlinks it instead,
/// which is what Unix does. This test found that difference on CI and
/// now pins the fix.
#[test]
fn replace_file_swaps_content_in_one_rename_and_leaves_no_stage_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("state.json");
    replace_file(&path, b"first").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"first");

    let reader = File::open(&path).unwrap();
    replace_file(&path, b"second").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"second");
    // What the held handle now sees is unlinked-file behaviour rather
    // than part of the contract, so it is not asserted; that it could
    // not block the replacement is the contract, and it just did not.
    drop(reader);

    let leftovers = |kept: &[&str]| -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| !kept.contains(&name.as_str()))
            .collect();
        names.sort();
        names
    };
    assert!(leftovers(&["state.json"]).is_empty(), "after a replacement");

    // A replacement that cannot complete leaves the old bytes in place:
    // a directory in the way is the cheapest way to make rename refuse
    // on every platform. Cleaning up the stage afterwards is ours to do
    // now that the rename is `fs::rename` and not a `tempfile` destructor.
    let blocked = tmp.path().join("blocked");
    replace_file(&blocked, b"original").unwrap();
    fs::remove_file(&blocked).unwrap();
    fs::create_dir(&blocked).unwrap();
    assert!(replace_file(&blocked, b"replacement").is_err());
    assert!(blocked.is_dir());
    assert!(
        leftovers(&["state.json", "blocked"]).is_empty(),
        "a failed replacement left its stage behind: {:?}",
        leftovers(&["state.json", "blocked"])
    );

    // The destination's parent must exist; nothing is created above it.
    assert!(replace_file(&tmp.path().join("missing").join("state.json"), b"x").is_err());
}

#[test]
fn lock_is_exclusive_across_handles() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = Archive::open(tmp.path()).unwrap();
    let held = archive.lock(|| panic!("should not wait")).unwrap();
    let other = File::open(tmp.path().join(LOCK_FILE)).unwrap();
    assert!(matches!(other.try_lock(), Err(TryLockError::WouldBlock)));
    drop(held);
    assert!(other.try_lock().is_ok());
}

#[test]
fn latest_matching_skips_unreadable_and_other_profiles() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = Archive::open(tmp.path()).unwrap();
    let make = |id: &str, body: &str| {
        let dir = archive.snapshots_dir().join(id);
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join(SNAPSHOT_JSON), body).unwrap();
    };
    let snapshot = |id: &str, profile: &str| {
        format!(
            r#"{{"schema_version":1,"id":"{id}","captured_at":"2026-01-01T00:00:00Z",
            "source":{{"browser":"chrome","profile":"{profile}","profile_display":null,"path":"/x",
            "file":"session.snss","sha256":"","bytes":0,"saved_at":null,"session_started_at":null}},
            "stats":{{"file_version":3,"command_table":"session","commands":0,"commands_by_id":{{}},
            "unknown_commands":0,"unknown_command_ids":[],"malformed_commands":0,"truncated_bytes":0,
            "marker_count":1,"marker_ok":true,"windows":0,"tabs":0,"groups":0,"dropped_tabs":0,
            "dropped_tab_reasons":{{"no_navigations":0,"window_missing":0,"window_closed":0}},
            "navigation_fallbacks":0,"groups_without_metadata":0}},"windows":[],"groups":[],"tabs":[]}}"#
        )
    };
    make(
        "2026-01-01-000001Z",
        &snapshot("2026-01-01-000001Z", "Default"),
    );
    make(
        "2026-01-01-000002Z",
        &snapshot("2026-01-01-000002Z", "Profile 1"),
    );
    make("2026-01-01-000003Z", "{not json");
    fs::create_dir(archive.snapshots_dir().join(".staging-x")).unwrap();

    let found = archive
        .latest_matching(Some("chrome"), Some("Default"))
        .unwrap();
    assert_eq!(found.snapshot.unwrap().id, "2026-01-01-000001Z");
    assert_eq!(found.unreadable.len(), 1);
    let none = archive.latest_matching(None, None).unwrap();
    assert!(none.snapshot.is_none());

    // Lexicographic ordering puts -9 after -10, which would compare the
    // new layout against the wrong snapshot after a busy capture burst.
    for suffix in [9, 10] {
        let id = format!("2026-01-01-000004Z-{suffix}");
        make(&id, &snapshot(&id, "Default"));
    }
    let latest = archive
        .latest_matching(Some("chrome"), Some("Default"))
        .unwrap();
    assert_eq!(latest.snapshot.unwrap().id, "2026-01-01-000004Z-10");
}
