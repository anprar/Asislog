// English comments: read-only mmap opening helper.

use std::path::Path;

/// Open a file read-only as memory map.
/// Caller must handle empty files (mmap of 0 bytes fails) separately.
pub fn open_mmap(path: &Path) -> std::io::Result<memmap2::Mmap> {
    let file = std::fs::File::open(path)?;
    // SAFETY: file is read-only, never written through this mapping.
    unsafe { memmap2::Mmap::map(&file) }
}
