use camino::Utf8Path;
use djls_source::ChangeEvent;
use djls_source::Db as _;
use djls_source::File;
use djls_source::FileError;
use djls_source::FileReadErrorKind;
use djls_source::FileStatus;
use djls_source::SourceChanges;
use djls_source::path_to_file;
use djls_testing::SalsaEventLog;
use djls_testing::TestDatabase;
use salsa::Database as _;

#[salsa::input]
struct LookupPath {
    #[returns(ref)]
    path: String,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum LookupOutcome {
    Ok,
    IsADirectory,
    NotFound,
}

#[salsa::tracked(returns(copy))]
fn lookup_outcome(db: &dyn djls_source::Db, lookup: LookupPath) -> LookupOutcome {
    match path_to_file(db, Utf8Path::new(lookup.path(db))) {
        Ok(_) => LookupOutcome::Ok,
        Err(FileError::IsADirectory) => LookupOutcome::IsADirectory,
        Err(FileError::NotFound) => LookupOutcome::NotFound,
    }
}

fn execution_count(db: &TestDatabase, events: &[salsa::Event], query_name: &str) -> usize {
    events
        .iter()
        .filter(|event| match &event.kind {
            salsa::EventKind::WillExecute { database_key } => db
                .ingredient_debug_name(database_key.ingredient_index())
                .contains(query_name),
            salsa::EventKind::DidValidateMemoizedValue { .. }
            | salsa::EventKind::WillBlockOn { .. }
            | salsa::EventKind::WillIterateCycle { .. }
            | salsa::EventKind::DidFinalizeCycle { .. }
            | salsa::EventKind::WillCheckCancellation
            | salsa::EventKind::DidSetCancellationFlag
            | salsa::EventKind::WillDiscardStaleOutput { .. }
            | salsa::EventKind::DidDiscard { .. }
            | salsa::EventKind::DidDiscardAccumulated { .. }
            | salsa::EventKind::DidInternValue { .. }
            | salsa::EventKind::DidReuseInternedValue { .. }
            | salsa::EventKind::DidValidateInternedValue { .. } => false,
        })
        .count()
}

#[test]
fn ancestor_lookup_reexecutes_after_child_file_is_created_and_synced() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let parent_path = Utf8Path::new("/project/foo");
    let child_path = Utf8Path::new("/project/foo/bar.py");

    let lookup = LookupPath::new(&db, parent_path.to_string());
    assert_eq!(lookup_outcome(&db, lookup), LookupOutcome::NotFound);
    event_log
        .take()
        .expect("initial ancestor lookup events should be cleared");

    db.add_file(child_path.as_str(), "print('created')\n")
        .expect("child fixture file should be added");
    File::sync_path(&mut db, child_path);

    let parent = db
        .files()
        .try_file(parent_path)
        .expect("parent path should have been interned by the lookup");
    assert_eq!(parent.status(&db), FileStatus::IsADirectory);
    assert_eq!(lookup_outcome(&db, lookup), LookupOutcome::IsADirectory);
    let events = event_log
        .take()
        .expect("ancestor lookup Salsa events should be read");
    assert!(execution_count(&db, &events, "lookup_outcome") > 0);
}

#[test]
fn path_to_file_missing_reexecutes_after_file_is_created_and_synced() {
    let mut db = TestDatabase::new();
    let path = "/project/app.py";

    let lookup = LookupPath::new(&db, path.to_string());
    assert_eq!(lookup_outcome(&db, lookup), LookupOutcome::NotFound);

    db.add_file(path, "print('created')\n")
        .expect("created fixture file should be added");
    File::sync_path(&mut db, Utf8Path::new(path));

    assert_eq!(lookup_outcome(&db, lookup), LookupOutcome::Ok);
}

#[test]
fn path_to_file_directory_returns_is_a_directory() {
    let db = TestDatabase::new();
    let path = "/project/pkg";
    db.add_file("/project/pkg/module.py", "")
        .expect("module fixture file should be added");

    assert_eq!(
        path_to_file(&db, Utf8Path::new(path)),
        Err(FileError::IsADirectory)
    );
}

#[test]
fn path_to_file_existing_file_returns_file_with_source() {
    let db = TestDatabase::new();
    let path = "/project/app.py";
    db.add_file(path, "print('hello')\n")
        .expect("source fixture file should be added");

    let file = path_to_file(&db, Utf8Path::new(path)).expect("file should exist");

    assert_eq!(
        file.try_source(&db)
            .expect("file should be readable")
            .as_str(),
        "print('hello')\n"
    );
}

#[test]
fn unregistered_file_reports_missing_synchronized_entry() {
    let db = TestDatabase::new();
    let path = Utf8Path::new("/project/app.py");
    db.add_file(path.as_str(), "print('hello')\n")
        .expect("source fixture file should be added");
    let file = File::builder(path.to_owned(), 0, FileStatus::Exists)
        .durability(salsa::Durability::LOW)
        .path_durability(salsa::Durability::HIGH)
        .new(&db);

    let error = file
        .try_source(&db)
        .expect_err("unregistered file should not have a synchronized source");

    assert_eq!(error.path(), path);
    assert_eq!(error.kind(), FileReadErrorKind::MissingSynchronizedEntry);
}

#[test]
fn readable_empty_source_is_distinct_from_unreadable_source() {
    let mut db = TestDatabase::new();
    let path = Utf8Path::new("/project/empty.py");
    db.add_file(path.as_str(), "")
        .expect("empty fixture file should be added");
    let file = path_to_file(&db, path).expect("file should exist");

    assert_eq!(
        file.try_source(&db)
            .expect("file should be readable")
            .as_str(),
        ""
    );

    db.remove_file(path.as_str())
        .expect("empty fixture file should be removed");
    SourceChanges::new([ChangeEvent::Rescan]).apply(&mut db);
    let error = file
        .try_source(&db)
        .expect_err("deleted file should be unreadable");
    assert_eq!(error.path(), path);
    assert_eq!(
        error.kind(),
        FileReadErrorKind::Filesystem(std::io::ErrorKind::NotFound)
    );

    db.add_file(path.as_str(), "recreated")
        .expect("deleted fixture file should be recreated");
    SourceChanges::new([ChangeEvent::Rescan]).apply(&mut db);
    assert_eq!(
        file.try_source(&db)
            .expect("recreated file should be readable")
            .as_str(),
        "recreated"
    );
}

#[test]
fn rescan_refreshes_content_and_backdates_equal_outcomes() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let path = Utf8Path::new("/project/app.py");
    db.add_file(path.as_str(), "old")
        .expect("initial fixture file should be added");
    let file = path_to_file(&db, path).expect("file should exist");
    assert_eq!(
        file.try_source(&db)
            .expect("initial file should be readable")
            .as_str(),
        "old"
    );

    let old_revision = file.revision(&db);
    db.add_file(path.as_str(), "new")
        .expect("updated fixture file should be added");
    SourceChanges::new([ChangeEvent::Rescan]).apply(&mut db);
    assert_eq!(file.revision(&db), old_revision + 1);
    assert_eq!(
        file.try_source(&db)
            .expect("updated file should be readable after rescan")
            .as_str(),
        "new"
    );

    let unchanged_revision = file.revision(&db);
    event_log
        .take()
        .expect("content update events should be cleared");
    SourceChanges::new([ChangeEvent::Rescan]).apply(&mut db);
    assert_eq!(file.revision(&db), unchanged_revision);
    assert_eq!(
        file.try_source(&db)
            .expect("unchanged file should remain readable after rescan")
            .as_str(),
        "new"
    );
    let events = event_log
        .take()
        .expect("unchanged rescan Salsa events should be read");
    assert_eq!(execution_count(&db, &events, "try_source"), 0);
}

#[test]
fn content_change_synchronizes_source_with_one_revision_bump() {
    let mut db = TestDatabase::new();
    let path = Utf8Path::new("/project/app.py");
    db.add_file(path.as_str(), "old")
        .expect("initial fixture file should be added");
    let file = path_to_file(&db, path).expect("file should exist");
    let old_revision = file.revision(&db);

    db.add_file(path.as_str(), "new")
        .expect("updated fixture file should be added");
    SourceChanges::new([ChangeEvent::ContentChanged(path.to_path_buf())]).apply(&mut db);

    assert_eq!(file.revision(&db), old_revision + 1);
    assert_eq!(
        file.try_source(&db)
            .expect("changed file should be readable after synchronization")
            .as_str(),
        "new"
    );
}

#[test]
fn sync_path_for_untracked_path_is_noop() {
    let mut db = TestDatabase::new();
    let path = Utf8Path::new("/project/never.py");

    File::sync_path(&mut db, path);

    assert!(db.files().try_file(path).is_none());
}

#[test]
fn path_to_file_deletion_invalidates_dependent_lookup() {
    let mut db = TestDatabase::new();
    let path = "/project/app.py";
    db.add_file(path, "print('present')\n")
        .expect("source fixture file should be added");

    let lookup = LookupPath::new(&db, path.to_string());
    assert_eq!(lookup_outcome(&db, lookup), LookupOutcome::Ok);

    db.remove_file(path)
        .expect("source fixture file should be removed");
    File::sync_path(&mut db, Utf8Path::new(path));

    assert_eq!(lookup_outcome(&db, lookup), LookupOutcome::NotFound);
}
