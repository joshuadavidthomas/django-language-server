//! File system watching for VFS synchronization.
//!
//! This module provides file system watching capabilities to detect external changes
//! and synchronize them with the VFS. It uses cross-platform file watching with
//! debouncing to handle rapid changes efficiently.

use anyhow::{anyhow, Result};
use camino::Utf8PathBuf;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::HashMap,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

/// Event types that can occur in the file system.
///
/// [`WatchEvent`] represents the different types of file system changes that
/// the watcher can detect and process.
#[derive(Clone, Debug, PartialEq)]
pub enum WatchEvent {
    /// A file was modified (content changed)
    Modified(Utf8PathBuf),
    /// A new file was created
    Created(Utf8PathBuf),
    /// A file was deleted
    Deleted(Utf8PathBuf),
    /// A file was renamed from one path to another
    Renamed { from: Utf8PathBuf, to: Utf8PathBuf },
}

/// Configuration for the file watcher.
///
/// [`WatchConfig`] controls how the file watcher operates, including what
/// directories to watch and how to filter events.
#[derive(Clone, Debug)]
pub struct WatchConfig {
    /// Whether file watching is enabled
    pub enabled: bool,
    /// Root directories to watch recursively
    pub roots: Vec<Utf8PathBuf>,
    /// Debounce time in milliseconds (collect events for this duration before processing)
    pub debounce_ms: u64,
    /// File patterns to include (e.g., ["*.py", "*.html"])
    pub include_patterns: Vec<String>,
    /// File patterns to exclude (e.g., ["__pycache__", ".git", "*.pyc"])
    pub exclude_patterns: Vec<String>,
}

// TODO: Allow for user config instead of hardcoding defaults
impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            roots: Vec::new(),
            debounce_ms: 250,
            include_patterns: vec!["*.py".to_string(), "*.html".to_string()],
            exclude_patterns: vec![
                "__pycache__".to_string(),
                ".git".to_string(),
                ".pyc".to_string(),
                "node_modules".to_string(),
                ".venv".to_string(),
                "venv".to_string(),
            ],
        }
    }
}

/// File system watcher for VFS synchronization.
///
/// [`VfsWatcher`] monitors the file system for changes and provides a channel
/// for consuming batched events. It handles debouncing and filtering internally.
pub struct VfsWatcher {
    /// The underlying file system watcher
    _watcher: RecommendedWatcher,
    /// Receiver for processed watch events
    rx: mpsc::Receiver<Vec<WatchEvent>>,
    /// Configuration for the watcher
    config: WatchConfig,
    /// Handle to the background processing thread
    _handle: thread::JoinHandle<()>,
}

impl VfsWatcher {
    /// Create a new file watcher with the given configuration.
    ///
    /// This starts watching the specified root directories and begins processing
    /// events in a background thread.
    pub fn new(config: WatchConfig) -> Result<Self> {
        if !config.enabled {
            return Err(anyhow!("File watching is disabled"));
        }

        let (event_tx, event_rx) = mpsc::channel();
        let (watch_tx, watch_rx) = mpsc::channel();

        // Create the file system watcher
        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                if let Ok(event) = res {
                    let _ = event_tx.send(event);
                }
            },
            Config::default(),
        )?;

        // Watch all root directories
        for root in &config.roots {
            let std_path = root.as_std_path();
            if std_path.exists() {
                watcher.watch(std_path, RecursiveMode::Recursive)?;
            }
        }

        // Spawn background thread for event processing
        let config_clone = config.clone();
        let handle = thread::spawn(move || {
            Self::process_events(&event_rx, &watch_tx, &config_clone);
        });

        Ok(Self {
            _watcher: watcher,
            rx: watch_rx,
            config,
            _handle: handle,
        })
    }

    /// Get the next batch of processed watch events.
    ///
    /// This is a non-blocking operation that returns immediately. If no events
    /// are available, it returns an empty vector.
    #[must_use]
    pub fn try_recv_events(&self) -> Vec<WatchEvent> {
        self.rx.try_recv().unwrap_or_default()
    }

    /// Background thread function for processing raw file system events.
    ///
    /// This function handles debouncing, filtering, and batching of events before
    /// sending them to the main thread for VFS synchronization.
    fn process_events(
        event_rx: &mpsc::Receiver<Event>,
        watch_tx: &mpsc::Sender<Vec<WatchEvent>>,
        config: &WatchConfig,
    ) {
        let mut pending_events: HashMap<Utf8PathBuf, WatchEvent> = HashMap::new();
        let mut last_batch_time = Instant::now();
        let debounce_duration = Duration::from_millis(config.debounce_ms);

        loop {
            // Try to receive events with a timeout for batching
            match event_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(event) => {
                    // Process the raw notify event into our WatchEvent format
                    if let Some(watch_events) = Self::convert_notify_event(event, config) {
                        for watch_event in watch_events {
                            let path = Self::get_event_path(&watch_event);
                            // Only keep the latest event for each path
                            pending_events.insert(path.clone(), watch_event);
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Timeout - check if we should flush pending events
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // Channel disconnected, exit the thread
                    break;
                }
            }

            // Check if we should flush pending events
            if !pending_events.is_empty() && last_batch_time.elapsed() >= debounce_duration {
                let events: Vec<WatchEvent> = pending_events.values().cloned().collect();
                if watch_tx.send(events).is_err() {
                    // Main thread disconnected, exit
                    break;
                }
                pending_events.clear();
                last_batch_time = Instant::now();
            }
        }
    }

    /// Convert a [`notify::Event`] into our [`WatchEvent`] format.
    fn convert_notify_event(event: Event, config: &WatchConfig) -> Option<Vec<WatchEvent>> {
        let mut watch_events = Vec::new();

        for path in event.paths {
            if let Ok(utf8_path) = Utf8PathBuf::try_from(path) {
                if Self::should_include_path_static(&utf8_path, config) {
                    match event.kind {
                        EventKind::Create(_) => watch_events.push(WatchEvent::Created(utf8_path)),
                        EventKind::Modify(_) => watch_events.push(WatchEvent::Modified(utf8_path)),
                        EventKind::Remove(_) => watch_events.push(WatchEvent::Deleted(utf8_path)),
                        _ => {} // Ignore other event types for now
                    }
                }
            }
        }

        if watch_events.is_empty() {
            None
        } else {
            Some(watch_events)
        }
    }

    /// Static version of should_include_path for use in convert_notify_event.
    fn should_include_path_static(path: &Utf8PathBuf, config: &WatchConfig) -> bool {
        let path_str = path.as_str();

        // Check exclude patterns first
        for pattern in &config.exclude_patterns {
            if path_str.contains(pattern) {
                return false;
            }
        }

        // If no include patterns, include everything (that's not excluded)
        if config.include_patterns.is_empty() {
            return true;
        }

        // Check include patterns
        for pattern in &config.include_patterns {
            if let Some(extension) = pattern.strip_prefix("*.") {
                if path_str.ends_with(extension) {
                    return true;
                }
            } else if path_str.contains(pattern) {
                return true;
            }
        }

        false
    }

    /// Extract the path from a [`WatchEvent`].
    fn get_event_path(event: &WatchEvent) -> &Utf8PathBuf {
        match event {
            WatchEvent::Modified(path) | WatchEvent::Created(path) | WatchEvent::Deleted(path) => {
                path
            }
            WatchEvent::Renamed { to, .. } => to,
        }
    }
}

impl Drop for VfsWatcher {
    fn drop(&mut self) {
        // The background thread will exit when the event channel is dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_watch_config_default() {
        let config = WatchConfig::default();
        assert!(config.enabled);
        assert_eq!(config.debounce_ms, 250);
        assert!(config.include_patterns.contains(&"*.py".to_string()));
        assert!(config.exclude_patterns.contains(&".git".to_string()));
    }

    #[test]
    fn test_should_include_path() {
        let config = WatchConfig::default();

        // Should include Python files
        assert!(VfsWatcher::should_include_path_static(
            &Utf8PathBuf::from("test.py"),
            &config
        ));

        // Should include HTML files
        assert!(VfsWatcher::should_include_path_static(
            &Utf8PathBuf::from("template.html"),
            &config
        ));

        // Should exclude .git files
        assert!(!VfsWatcher::should_include_path_static(
            &Utf8PathBuf::from(".git/config"),
            &config
        ));

        // Should exclude __pycache__ files
        assert!(!VfsWatcher::should_include_path_static(
            &Utf8PathBuf::from("__pycache__/test.pyc"),
            &config
        ));
    }

    #[test]
    fn test_watch_event_types() {
        let path1 = Utf8PathBuf::from("test.py");
        let path2 = Utf8PathBuf::from("new.py");

        let modified = WatchEvent::Modified(path1.clone());
        let created = WatchEvent::Created(path1.clone());
        let deleted = WatchEvent::Deleted(path1.clone());
        let renamed = WatchEvent::Renamed {
            from: path1,
            to: path2,
        };

        // Test that events can be created and compared
        assert_ne!(modified, created);
        assert_ne!(created, deleted);
        assert_ne!(deleted, renamed);
    }
}
