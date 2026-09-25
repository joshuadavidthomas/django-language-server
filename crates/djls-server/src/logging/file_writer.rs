use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::sync::Arc;
use std::sync::PoisonError;
use std::sync::RwLock;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::mpsc::SyncSender;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::SystemTime;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use tracing_subscriber::fmt::MakeWriter;

const FILE_LIMIT: u64 = 16 * 1024 * 1024;
const RECORD_LIMIT: usize = 16 * 1024;
const QUEUE_LIMIT: usize = 1024;
const NAMES: [&str; 4] = [
    "djls-bounded.log",
    "djls-bounded.log.1",
    "djls-bounded.log.2",
    "djls-bounded.log.3",
];
const LOCK: &str = "djls-bounded.lock";
// An older server may still be writing its current daily file.
const LEGACY_GRACE: Duration = Duration::from_hours(24);

#[derive(Default)]
pub(super) struct Counters {
    oversized: AtomicUsize,
    queue: AtomicUsize,
    io_failures: AtomicUsize,
    unavailable: AtomicUsize,
}

fn count(counter: &AtomicUsize) {
    let _previous = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
        Some(n.saturating_add(1))
    });
}

type Admission = Arc<RwLock<Option<SyncSender<Vec<u8>>>>>;

#[derive(Clone)]
pub(super) struct RecordWriter {
    sender: Admission,
    counters: Arc<Counters>,
}

impl RecordWriter {
    pub(super) fn start(
        mut sink: impl Write + Send + 'static,
        counters: Arc<Counters>,
    ) -> (Self, WorkerGuard) {
        let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(QUEUE_LIMIT);
        let sender = Arc::new(RwLock::new(Some(sender)));
        let worker = thread::Builder::new()
            .name("djls-logging".into())
            .spawn(move || {
                for bytes in receiver {
                    let _written = sink.write_all(&bytes);
                }
                let _flushed = sink.flush();
            });
        let worker = match worker {
            Ok(worker) => Some(worker),
            Err(error) => {
                sender
                    .write()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                let _warned = writeln!(
                    io::stderr(),
                    "Warning: DJLS logging worker unavailable: {error}"
                );
                None
            }
        };
        let guard = WorkerGuard {
            sender: Arc::clone(&sender),
            worker,
        };
        (Self { sender, counters }, guard)
    }
}

pub(crate) struct WorkerGuard {
    sender: Admission,
    worker: Option<JoinHandle<()>>,
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        // The only sender lives behind this lock. Closing admission disconnects
        // the channel even while the subscriber retains RecordWriter clones.
        // No shutdown message competes for space in the saturated queue.
        self.sender
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(worker) = self.worker.take() {
            let _joined = worker.join();
        }
    }
}

pub(super) struct Record {
    output: RecordWriter,
    bytes: Vec<u8>,
    oversized: bool,
}

impl<'a> MakeWriter<'a> for RecordWriter {
    type Writer = Record;

    fn make_writer(&'a self) -> Record {
        Record {
            output: self.clone(),
            bytes: Vec::new(),
            oversized: false,
        }
    }
}

impl Write for Record {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if !self.oversized {
            if bytes.len() > RECORD_LIMIT - self.bytes.len() {
                self.oversized = true;
                self.bytes.clear();
                count(&self.output.counters.oversized);
            } else {
                self.bytes.reserve_exact(bytes.len());
                self.bytes.extend_from_slice(bytes);
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        if !self.oversized && !self.bytes.is_empty() {
            // Producers share the read side, so they never wait on each other;
            // only shutdown takes the write side to close admission.
            let sender = self
                .output
                .sender
                .read()
                .unwrap_or_else(PoisonError::into_inner);
            match sender.as_ref() {
                Some(sender) => {
                    if sender.try_send(std::mem::take(&mut self.bytes)).is_err() {
                        count(&self.output.counters.queue);
                    }
                }
                None => count(&self.output.counters.unavailable),
            }
        }
    }
}

pub(super) struct FileWriter {
    directory: Option<Utf8PathBuf>,
    limit: u64,
    counters: Arc<Counters>,
    #[cfg(test)]
    fault: Option<Fault>,
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq)]
enum Fault {
    Rename,
    Open,
    PartialWrite,
}

impl FileWriter {
    pub(super) fn new(directory: io::Result<Utf8PathBuf>, counters: Arc<Counters>) -> Self {
        let mut writer = Self {
            directory: None,
            limit: FILE_LIMIT,
            counters,
            #[cfg(test)]
            fault: None,
        };
        match directory {
            Ok(path) => {
                remove_legacy_logs(&path);
                writer.directory = Some(path);
            }
            Err(error) => writer.disable(&error),
        }
        writer
    }

    fn disable(&mut self, error: &io::Error) {
        self.directory = None;
        count(&self.counters.io_failures);
        // No tracing: failure reporting must not recurse or touch LSP stdout.
        let _warned = writeln!(io::stderr(), "Warning: DJLS file logging disabled: {error}");
    }

    fn append(&self, directory: &Utf8Path, bytes: &[u8]) -> io::Result<()> {
        fs::create_dir_all(directory)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(directory.join(LOCK))?;
        // Waiting is safe here: this runs on the logging worker, and the lossy
        // queue in front of it absorbs a stalled peer process.
        lock.lock()?;
        // Metadata is authoritative, including after partial writes and restart.
        // Repair only this managed family, and never follow a log-file symlink.
        let mut active_len = 0;
        for (index, name) in NAMES.iter().enumerate() {
            let path = directory.join(name);
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if !metadata.is_file() {
                return Err(io::Error::other("managed log is not a regular file"));
            }
            if metadata.len() > self.limit {
                OpenOptions::new()
                    .write(true)
                    .open(&path)?
                    .set_len(self.limit)?;
            }
            if index == 0 {
                active_len = metadata.len().min(self.limit);
            }
        }
        if bytes.len() as u64 > self.limit - active_len {
            match fs::remove_file(directory.join(NAMES[3])) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            for index in (0..3).rev() {
                #[cfg(test)]
                if self.fault == Some(Fault::Rename) {
                    return Err(io::Error::other("injected rename failure"));
                }
                match fs::rename(
                    directory.join(NAMES[index]),
                    directory.join(NAMES[index + 1]),
                ) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
        // This handle is closed before the lock is released, on every path.
        #[cfg(test)]
        if self.fault == Some(Fault::Open) {
            return Err(io::Error::other("injected open failure"));
        }
        let mut active = OpenOptions::new()
            .create(true)
            .append(true)
            .open(directory.join(NAMES[0]))?;
        #[cfg(test)]
        if self.fault == Some(Fault::PartialWrite) {
            active.write_all(&bytes[..1])?;
            return Err(io::Error::other("injected failure after partial write"));
        }
        active.write_all(bytes)
    }
}

impl Write for FileWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > RECORD_LIMIT || bytes.len() as u64 > self.limit {
            count(&self.counters.oversized);
        } else if let Some(directory) = &self.directory {
            if let Err(error) = self.append(directory, bytes) {
                count(&self.counters.unavailable);
                self.disable(&error);
            }
        } else {
            count(&self.counters.unavailable);
        }
        // Swallow failures: the worker must never retry an already partial record.
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Removes the unbounded daily files written by releases before the bounded
/// family existed (#836). Failures are ignored: this is best-effort cleanup.
fn remove_legacy_logs(directory: &Utf8Path) {
    let Ok(entries) = directory.read_dir_utf8() else {
        return;
    };
    let cutoff = SystemTime::now().checked_sub(LEGACY_GRACE);
    for entry in entries.flatten() {
        if !is_legacy_log_name(entry.file_name()) {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let stale = cutoff
            .is_some_and(|cutoff| metadata.modified().is_ok_and(|modified| modified < cutoff));
        if metadata.is_file() && stale {
            let _removed = fs::remove_file(entry.path());
        }
    }
}

fn is_legacy_log_name(name: &str) -> bool {
    name.strip_prefix("djls.log.").is_some_and(|date| {
        date.len() == 10
            && date.bytes().enumerate().all(|(index, byte)| match index {
                4 | 7 => byte == b'-',
                _ => byte.is_ascii_digit(),
            })
    })
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::sync::Mutex;
    use std::sync::mpsc;

    use super::*;

    // A concurrent fork can briefly retain another test's locked descriptor
    // until exec closes it. Keep subprocess creation out of single-writer tests.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn boundaries_rollover_restart_and_unrelated_files() {
        let _serial = TEST_LOCK.lock().expect("test lock");
        let temp = tempfile::tempdir().expect("temporary directory");
        let path = Utf8Path::from_path(temp.path()).expect("UTF-8 path");
        fs::write(path.join("djls.log.2026-01-01"), b"legacy").expect("legacy log");
        fs::write(path.join("unrelated"), b"keep").expect("unrelated file");
        let counters = Arc::new(Counters::default());
        let mut writer = FileWriter::new(Ok(path.to_owned()), counters);
        writer.limit = 7;
        writer.write_all(b"abc").expect("write record");
        writer.write_all(b"defg").expect("write exact boundary");
        assert_eq!(
            fs::read(path.join(NAMES[0])).expect("read active"),
            b"abcdefg"
        );
        assert!(!path.join(NAMES[1]).exists());
        drop(writer);
        let mut writer = FileWriter::new(Ok(path.to_owned()), Arc::default());
        writer.limit = 7;
        for byte in b'h'..=b'z' {
            writer.write_all(&[byte; 7]).expect("roll over");
            let mut total = 0;
            for name in NAMES {
                let len = fs::metadata(path.join(name)).map_or(0, |m| m.len());
                assert!(len <= 7);
                total += len;
            }
            assert!(total <= 28);
        }
        for (name, byte) in NAMES.into_iter().zip(*b"zyxw") {
            assert_eq!(fs::read(path.join(name)).expect("read archive"), [byte; 7]);
        }
        drop(writer);
        for name in NAMES {
            fs::write(path.join(name), b"oversized!").expect("oversized fixture");
        }
        let mut writer = FileWriter::new(Ok(path.to_owned()), Arc::default());
        writer.limit = 7;
        writer.write_all(b"!").expect("write after restart");
        assert_eq!(fs::read(path.join(NAMES[0])).expect("read active"), b"!");
        for name in &NAMES[1..] {
            assert_eq!(fs::read(path.join(name)).expect("read archive"), b"oversiz");
        }
        assert_eq!(
            fs::read(path.join("unrelated")).expect("unrelated remains"),
            b"keep"
        );
        assert_eq!(
            fs::read(path.join("djls.log.2026-01-01")).expect("legacy remains"),
            b"legacy"
        );
    }

    #[test]
    fn filesystem_failure_releases_lock_and_disables_output() {
        let _serial = TEST_LOCK.lock().expect("test lock");
        for (fault, expected_restart) in [
            (Fault::Rename, &b"AB"[..]),
            (Fault::Open, &b"AB"[..]),
            (Fault::PartialWrite, &b"xAB"[..]),
        ] {
            let temp = tempfile::tempdir().expect("temporary directory");
            let path = Utf8Path::from_path(temp.path()).expect("UTF-8 path");
            let counters = Arc::new(Counters::default());
            let mut writer = FileWriter::new(Ok(path.to_owned()), Arc::clone(&counters));
            writer.limit = 7;
            writer.write_all(b"123456").expect("initial record");
            writer.fault = Some(fault);
            writer.write_all(b"xy").expect("failure is swallowed");
            assert!(writer.directory.is_none());
            writer.write_all(b"z").expect("disabled write");
            assert_eq!(counters.unavailable.load(Ordering::Relaxed), 2);
            assert_eq!(counters.io_failures.load(Ordering::Relaxed), 1);
            let lock = OpenOptions::new()
                .write(true)
                .open(path.join(LOCK))
                .expect("reopen lock");
            lock.try_lock().expect("failure released lock");
            drop(lock);
            let mut restarted = FileWriter::new(Ok(path.to_owned()), Arc::default());
            restarted.limit = 7;
            restarted.write_all(b"AB").expect("restart write");
            assert_eq!(
                fs::read(path.join(NAMES[0])).expect("read active"),
                expected_restart
            );
        }
    }

    #[test]
    fn legacy_daily_logs_are_removed_once_stale() {
        let _serial = TEST_LOCK.lock().expect("test lock");
        let temp = tempfile::tempdir().expect("temporary directory");
        let path = Utf8Path::from_path(temp.path()).expect("UTF-8 path");
        let old = SystemTime::now() - LEGACY_GRACE - Duration::from_mins(1);
        for name in [
            "djls.log.2026-01-01",
            "djls.log.2026-01-02",
            "djls.log.2026-01-01.bak",
            "djls.log.20260101",
            "djls-bounded.log.1",
            "unrelated",
        ] {
            fs::File::create(path.join(name))
                .expect("fixture")
                .set_modified(old)
                .expect("age fixture");
        }
        fs::write(path.join("djls.log.2026-09-24"), b"recent").expect("recent legacy log");

        drop(FileWriter::new(Ok(path.to_owned()), Arc::default()));

        let mut remaining: Vec<_> = fs::read_dir(path)
            .expect("read directory")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .into_string()
                    .expect("name")
            })
            .collect();
        remaining.sort();
        assert_eq!(
            remaining,
            [
                "djls-bounded.log.1",
                "djls.log.2026-01-01.bak",
                "djls.log.2026-09-24",
                "djls.log.20260101",
                "unrelated",
            ]
        );
    }

    #[test]
    fn oversized_records_never_enter_queue() {
        let _serial = TEST_LOCK.lock().expect("test lock");
        let temp = tempfile::tempdir().expect("temporary directory");
        let path = Utf8Path::from_path(temp.path()).expect("UTF-8 path");
        let counters = Arc::new(Counters::default());
        let (output, guard) = RecordWriter::start(
            FileWriter::new(Ok(path.to_owned()), Arc::clone(&counters)),
            Arc::clone(&counters),
        );
        {
            let mut record = output.make_writer();
            record
                .write_all(&vec![b'a'; RECORD_LIMIT])
                .expect("exact limit");
        }
        {
            let mut record = output.make_writer();
            record
                .write_all(&vec![b'b'; RECORD_LIMIT])
                .expect("limit prefix");
            record.write_all(b"x").expect("cross limit");
            record.write_all(b"y").expect("already oversized");
        }
        let subscriber = tracing_subscriber::fmt()
            .with_writer(output)
            .with_max_level(tracing::Level::TRACE)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            tracing::trace!(message = %"x".repeat(RECORD_LIMIT + 1));
        });
        drop(guard);
        assert_eq!(counters.oversized.load(Ordering::Relaxed), 2);
        assert_eq!(counters.queue.load(Ordering::Relaxed), 0);
        assert_eq!(
            fs::read(path.join(NAMES[0])).expect("read accepted record"),
            vec![b'a'; RECORD_LIMIT]
        );
    }

    #[test]
    fn concurrent_processes_share_one_family() {
        const CHILD: &str = "DJLS_LOGGING_CONCURRENT_CHILD";
        let _serial = TEST_LOCK.lock().expect("test lock");
        if let Ok(path) = std::env::var(CHILD) {
            let path = Utf8Path::new(&path);
            let mut writer = FileWriter::new(Ok(path.to_owned()), Arc::default());
            writer.limit = 31;
            for _ in 0..300 {
                writer.write_all(b"1234567").expect("concurrent write");
                let lock = OpenOptions::new()
                    .write(true)
                    .open(path.join(LOCK))
                    .expect("open lock");
                lock.lock().expect("lock inspection");
                let sizes: Vec<_> = NAMES
                    .iter()
                    .map(|name| fs::metadata(path.join(name)).map_or(0, |m| m.len()))
                    .collect();
                assert!(sizes.iter().all(|&size| size <= 31));
                assert!(sizes.iter().sum::<u64>() <= 124);
            }
            assert!(writer.directory.is_some());
            return;
        }
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut children = Vec::new();
        for _ in 0..4 {
            children.push(
                Command::new(std::env::current_exe().expect("test binary"))
                    .args([
                        "--exact",
                        "logging::file_writer::tests::concurrent_processes_share_one_family",
                    ])
                    .env(CHILD, temp.path())
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .expect("spawn writer"),
            );
        }
        for mut child in children {
            assert!(child.wait().expect("wait for writer").success());
        }
        let active = fs::read(temp.path().join(NAMES[0])).expect("read active");
        assert!(!active.is_empty());
        assert!(active.chunks(7).all(|record| record == b"1234567"));
    }

    #[test]
    fn persistent_failure_warns_once_and_keeps_stdout_clean() {
        const CHILD: &str = "DJLS_LOGGING_FAILURE_CHILD";
        let _serial = TEST_LOCK.lock().expect("test lock");
        if let Ok(path) = std::env::var(CHILD) {
            let path = Utf8Path::new(&path);
            // A directory in the managed family is a real filesystem failure,
            // even when tests run as root (permission tests would not be).
            fs::create_dir(path.join(NAMES[0])).expect("create obstruction");
            let counters = Arc::new(Counters::default());
            let mut writer = FileWriter::new(Ok(path.to_owned()), Arc::clone(&counters));
            for _ in 0..20 {
                writer.write_all(b"failed").expect("failure is swallowed");
            }
            assert_eq!(counters.io_failures.load(Ordering::Relaxed), 1);
            assert_eq!(counters.unavailable.load(Ordering::Relaxed), 20);
            assert!(path.join(NAMES[0]).is_dir());
            std::process::exit(0);
        }
        let temp = tempfile::tempdir().expect("temporary directory");
        let output = Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "logging::file_writer::tests::persistent_failure_warns_once_and_keeps_stdout_clean",
                "--nocapture",
            ])
            .env(CHILD, temp.path())
            .output()
            .expect("run failure child");
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout)
                .expect("UTF-8 stdout")
                .trim(),
            "running 1 test"
        );
        let stderr = String::from_utf8(output.stderr).expect("UTF-8 stderr");
        assert_eq!(stderr.lines().count(), 1);
        assert!(stderr.starts_with("Warning: DJLS file logging disabled:"));
    }

    struct BlockedWriter {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
        written: Arc<AtomicUsize>,
        flushed: Arc<AtomicUsize>,
    }

    impl Write for BlockedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.written.load(Ordering::Relaxed) == 0 {
                self.entered.send(()).map_err(io::Error::other)?;
                self.release.recv().map_err(io::Error::other)?;
            }
            count(&self.written);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            count(&self.flushed);
            Ok(())
        }
    }

    #[test]
    fn saturated_shutdown_keeps_stdout_clean() {
        const CHILD: &str = "DJLS_LOGGING_SHUTDOWN_CHILD";
        let _serial = TEST_LOCK.lock().expect("test lock");
        if std::env::var_os(CHILD).is_some() {
            let (entered_tx, entered_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let counters = Arc::new(Counters::default());
            let written = Arc::new(AtomicUsize::new(0));
            let flushed = Arc::new(AtomicUsize::new(0));
            let (writer, guard) = RecordWriter::start(
                BlockedWriter {
                    entered: entered_tx,
                    release: release_rx,
                    written: Arc::clone(&written),
                    flushed: Arc::clone(&flushed),
                },
                Arc::clone(&counters),
            );
            writer
                .make_writer()
                .write_all(b"in flight")
                .expect("in-flight record");
            entered_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("worker entered");
            for _ in 0..QUEUE_LIMIT {
                writer
                    .make_writer()
                    .write_all(b"queued")
                    .expect("queued record");
            }
            writer
                .make_writer()
                .write_all(b"dropped")
                .expect("lossy write");
            assert_eq!(counters.queue.load(Ordering::Relaxed), 1);
            let shutdown = thread::spawn(move || drop(guard));
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while writer.sender.read().expect("admission lock").is_some() {
                assert!(std::time::Instant::now() < deadline);
                thread::yield_now();
            }
            assert!(!shutdown.is_finished());
            writer
                .make_writer()
                .write_all(b"after shutdown")
                .expect("stopped write");
            assert_eq!(counters.unavailable.load(Ordering::Relaxed), 1);
            release_tx.send(()).expect("release worker");
            shutdown.join().expect("shutdown joined");
            assert_eq!(written.load(Ordering::Relaxed), QUEUE_LIMIT + 1);
            assert_eq!(flushed.load(Ordering::Relaxed), 1);
            std::process::exit(0);
        }
        let output = Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "logging::file_writer::tests::saturated_shutdown_keeps_stdout_clean",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .expect("run shutdown child");
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        // The test harness prints its preamble; any other stdout is a protocol violation.
        assert_eq!(
            String::from_utf8(output.stdout)
                .expect("UTF-8 stdout")
                .trim(),
            "running 1 test"
        );
    }
}
