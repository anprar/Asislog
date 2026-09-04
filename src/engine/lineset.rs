// English comments: compressed sorted set of 1-based line numbers.
// Backs Doc::filter_map (and future result-set composition): a filter
// matching 200M lines costs ~tens of MB here instead of 1.6 GB as Vec<u64>.

use roaring::RoaringBitmap;

/// Sorted set of 1-based line numbers, roaring-compressed.
/// Lines beyond u32::MAX (~4.29B, i.e. >150 GB of typical logs) cannot be
/// stored and are skipped by the extending constructors (documented, never
/// panics). All iterators yield ascending lines.
#[derive(Clone, Debug, Default)]
pub struct LineSet {
    inner: RoaringBitmap,
}

impl LineSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build from line numbers (any order; sorted runs compress best).
    /// Sorts + dedups once, bulk-loads, then optimizes to run containers,
    /// so dense filters land in KBs, not MBs.
    pub fn from_lines(it: impl IntoIterator<Item = u64>) -> Self {
        let mut v: Vec<u32> = it.into_iter().filter_map(|l| u32::try_from(l).ok()).collect();
        v.sort_unstable();
        v.dedup();
        let mut inner: RoaringBitmap = v.into_iter().collect();
        inner.optimize();
        Self { inner }
    }

    /// Number of stored lines.
    pub fn len(&self) -> u64 {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Insert more lines (any order; ascending runs compress best).
    pub fn extend(&mut self, it: impl IntoIterator<Item = u64>) {
        self.inner.extend(it.into_iter().filter_map(|l| u32::try_from(l).ok()));
    }

    /// Replace the whole set.
    pub fn replace_with(&mut self, it: impl IntoIterator<Item = u64>) {
        *self = Self::from_lines(it);
    }

    /// Remove one line. True when it was present.
    pub fn remove(&mut self, line: u64) -> bool {
        u32::try_from(line).map(|l| self.inner.remove(l)).unwrap_or(false)
    }

    /// View-row -> original line: the row-th smallest stored line.
    pub fn row_to_line(&self, row: u64) -> Option<u64> {
        let n = u32::try_from(row).ok()?;
        self.inner.select(n).map(u64::from)
    }

    /// Original line -> view-row (0-based) when the line is stored.
    pub fn line_to_row(&self, line: u64) -> Option<u64> {
        let l = u32::try_from(line).ok()?;
        if !self.inner.contains(l) {
            return None;
        }
        Some(self.inner.rank(l).saturating_sub(1))
    }

    /// Ascending line iterator.
    pub fn iter(&self) -> impl Iterator<Item = u64> + '_ {
        self.inner.iter().map(u64::from)
    }

    /// Exact serialized byte size (for memory accounting).
    pub fn heap_bytes(&self) -> usize {
        self.inner.serialized_size()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_basic() {
        let mut s = LineSet::new();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
        assert_eq!(s.row_to_line(0), None);
        assert_eq!(s.line_to_row(1), None);
        s.extend([3u64, 1, 2]);
        assert_eq!(s.len(), 3);
        assert_eq!(s.row_to_line(0), Some(1));
        assert_eq!(s.row_to_line(2), Some(3));
        assert_eq!(s.row_to_line(3), None);
        assert_eq!(s.line_to_row(2), Some(1));
        assert_eq!(s.line_to_row(9), None);
        assert!(s.remove(2));
        assert!(!s.remove(2));
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![1, 3]);
        s.clear();
        assert!(s.is_empty());
    }

    #[test]
    fn dense_run_compresses() {
        // 500k consecutive lines: Vec<u64> would need 4 MB.
        // (Kept modest so debug-mode CI stays fast; scales linearly.)
        let s = LineSet::from_lines(1..=500_000u64);
        assert_eq!(s.len(), 500_000);
        assert_eq!(s.row_to_line(499_999), Some(500_000));
        assert_eq!(s.line_to_row(500_000), Some(499_999));
        assert!(
            s.heap_bytes() < 5_000,
            "bytes={} (want run containers, ~KBs)",
            s.heap_bytes()
        );
    }

    #[test]
    fn sparse_stays_correct() {
        let want: Vec<u64> = (0..100_000u64).map(|i| i * 1000 + 7).collect();
        let s = LineSet::from_lines(want.iter().copied());
        assert_eq!(s.len(), 100_000);
        assert_eq!(s.iter().collect::<Vec<_>>(), want);
        assert_eq!(s.line_to_row(7), Some(0));
        assert_eq!(s.line_to_row(100_000 * 1000 + 7 - 1000), Some(99_999));
        assert_eq!(s.line_to_row(8), None);
    }

    #[test]
    fn huge_lines_capped_without_panic() {
        let mut s = LineSet::new();
        s.extend([1u64, u64::MAX, u32::MAX as u64]);
        // u64::MAX skipped; u32::MAX stored.
        assert_eq!(s.len(), 2);
        assert_eq!(s.row_to_line(1), Some(u32::MAX as u64));
    }

    #[test]
    fn replace_with_resets() {
        let mut s = LineSet::from_lines([1u64, 2, 3]);
        s.replace_with([10u64, 20]);
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![10, 20]);
    }
}
