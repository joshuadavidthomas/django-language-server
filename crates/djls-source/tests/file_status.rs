use camino::Utf8Path;
use djls_source::Db as _;
use djls_source::File;
use djls_source::FileError;
use djls_source::path_to_file;
use djls_testing::TestDatabase;

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

#[salsa::tracked]
fn lookup_outcome(db: &dyn djls_source::Db, lookup: LookupPath) -> LookupOutcome {
    match path_to_file(db, Utf8Path::new(lookup.path(db))) {
        Ok(_) => LookupOutcome::Ok,
        Err(FileError::IsADirectory) => LookupOutcome::IsADirectory,
        Err(FileError::NotFound) => LookupOutcome::NotFound,
    }
}

#[test]
fn path_to_file_missing_reexecutes_after_file_is_created_and_synced() {
    let mut db = TestDatabase::new();
    let path = "/project/app.py";

    let lookup = LookupPath::new(&db, path.to_string());
    assert_eq!(lookup_outcome(&db, lookup), LookupOutcome::NotFound);

    db.add_file(path, "print('created')\n");
    File::sync_path(&mut db, Utf8Path::new(path));

    assert_eq!(lookup_outcome(&db, lookup), LookupOutcome::Ok);
}

#[test]
fn path_to_file_directory_returns_is_a_directory() {
    let db = TestDatabase::new();
    let path = "/project/pkg";
    db.add_file("/project/pkg/module.py", "");

    assert_eq!(
        path_to_file(&db, Utf8Path::new(path)),
        Err(FileError::IsADirectory)
    );
}

#[test]
fn path_to_file_existing_file_returns_file_with_source() {
    let db = TestDatabase::new();
    let path = "/project/app.py";
    db.add_file(path, "print('hello')\n");

    let file = path_to_file(&db, Utf8Path::new(path)).expect("file should exist");

    assert_eq!(file.source(&db).as_str(), "print('hello')\n");
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
    db.add_file(path, "print('present')\n");

    let lookup = LookupPath::new(&db, path.to_string());
    assert_eq!(lookup_outcome(&db, lookup), LookupOutcome::Ok);

    db.remove_file(path);
    File::sync_path(&mut db, Utf8Path::new(path));

    assert_eq!(lookup_outcome(&db, lookup), LookupOutcome::NotFound);
}
