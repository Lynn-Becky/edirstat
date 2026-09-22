use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use arc_swap::ArcSwap;
use compact_str::CompactString;

use crate::arena::{FileArenaSnapshot, NodeStorage, StringPool};

/// Live counters describing scan progress, shared between the traversal engine
/// and any frontends displaying them.
#[derive(Debug, Clone, Default)]
pub struct TraversalStats {
    pub files_scanned: Arc<AtomicUsize>,
    pub dirs_scanned: Arc<AtomicUsize>,
    pub bytes_scanned: Arc<AtomicUsize>,
    /// 0 = pending, 1 = raw MFT, 2 = directory walker.
    pub backend: Arc<AtomicUsize>,
    /// Set when raw MFT was attempted but the walker had to take over.
    pub mft_fallback: Arc<AtomicBool>,
}

impl TraversalStats {
    pub fn reset(&self) {
        self.files_scanned.store(0, Ordering::SeqCst);
        self.dirs_scanned.store(0, Ordering::SeqCst);
        self.bytes_scanned.store(0, Ordering::SeqCst);
        self.backend.store(0, Ordering::SeqCst);
        self.mft_fallback.store(false, Ordering::SeqCst);
    }
}

#[derive(Debug)]
pub struct SharedState {
    /// Atomic pointer to the latest immutable snapshot of the tree
    pub current_snapshot: ArcSwap<FileArenaSnapshot>,
    /// Indicates whether the scanner is actively running
    pub is_scanning: Arc<AtomicBool>,
    /// Indicates whether a cancellation request has been triggered
    pub scan_cancel: Arc<AtomicBool>,
    /// Background-computed live extension statistics (ext, `total_size`, `file_count`)
    pub extension_stats: ArcSwap<Vec<(CompactString, u64, u32)>>,
    /// Live scan progress counters (files/dirs/bytes scanned)
    pub scan_stats: TraversalStats,
}

impl Default for SharedState {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedState {
    #[must_use]
    pub fn new() -> Self {
        let initial_snapshot = FileArenaSnapshot {
            nodes: Arc::new(NodeStorage::Owned(Vec::new())),
            string_pool: Arc::new(StringPool::new()),
            dir_counts: Arc::new(Vec::new()),
        };
        Self {
            current_snapshot: ArcSwap::new(Arc::new(initial_snapshot)),
            is_scanning: Arc::new(AtomicBool::new(false)),
            scan_cancel: Arc::new(AtomicBool::new(false)),
            extension_stats: ArcSwap::new(Arc::new(Vec::new())),
            scan_stats: TraversalStats::default(),
        }
    }

    /// Publish a new immutable snapshot atomically.
    pub fn store_snapshot(&self, snapshot: FileArenaSnapshot) {
        self.current_snapshot.store(Arc::new(snapshot));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::{FileNode, StringId};

    #[test]
    fn test_traversal_stats_default_zeroed() {
        let stats = TraversalStats::default();
        assert_eq!(stats.files_scanned.load(Ordering::SeqCst), 0);
        assert_eq!(stats.dirs_scanned.load(Ordering::SeqCst), 0);
        assert_eq!(stats.bytes_scanned.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_traversal_stats_clone_shares_counters() {
        let stats = TraversalStats::default();
        let clone = stats.clone();

        stats.files_scanned.fetch_add(7, Ordering::SeqCst);
        clone.dirs_scanned.fetch_add(3, Ordering::SeqCst);
        stats.bytes_scanned.fetch_add(1024, Ordering::SeqCst);

        assert_eq!(clone.files_scanned.load(Ordering::SeqCst), 7);
        assert_eq!(stats.dirs_scanned.load(Ordering::SeqCst), 3);
        assert_eq!(clone.bytes_scanned.load(Ordering::SeqCst), 1024);
    }

    #[test]
    fn test_traversal_stats_reset_zeroes_counters() {
        let stats = TraversalStats::default();
        stats.files_scanned.fetch_add(5, Ordering::SeqCst);
        stats.dirs_scanned.fetch_add(2, Ordering::SeqCst);
        stats.bytes_scanned.fetch_add(4096, Ordering::SeqCst);

        stats.reset();

        assert_eq!(stats.files_scanned.load(Ordering::SeqCst), 0);
        assert_eq!(stats.dirs_scanned.load(Ordering::SeqCst), 0);
        assert_eq!(stats.bytes_scanned.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_shared_state_new_is_empty() {
        let state = SharedState::new();
        assert_eq!(state.current_snapshot.load().nodes.len(), 0);
        assert!(!state.is_scanning.load(Ordering::SeqCst));
        assert!(!state.scan_cancel.load(Ordering::SeqCst));
        assert!(state.extension_stats.load().is_empty());
    }

    #[test]
    fn test_shared_state_store_snapshot_swaps() {
        let state = SharedState::new();
        let before = state.current_snapshot.load();
        assert_eq!(before.nodes.len(), 0);

        let snapshot = FileArenaSnapshot {
            nodes: Arc::new(NodeStorage::Owned(vec![FileNode::new(
                StringId(0),
                None,
                true,
                false,
                0,
                0,
            )])),
            string_pool: Arc::new(StringPool::new()),
            dir_counts: Arc::new(vec![0]),
        };
        state.store_snapshot(snapshot);

        let after = state.current_snapshot.load();
        assert_eq!(after.nodes.len(), 1);
        // Arc-swap semantics: a guard loaded before the store still sees the old snapshot.
        assert_eq!(before.nodes.len(), 0);
    }
}
