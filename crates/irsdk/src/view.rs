//! Where bytes come from: a trait over the live map, files and synthetic images.

/// A readable region of bytes that may change while it is read (the live map) or not (a file).
///
/// The parsers never form a slice over a live region; they copy out of it first.
pub trait View {
    /// Length of the region in bytes.
    fn len(&self) -> usize;

    /// Whether the region is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Loads the little-endian `i32` at `off`; `None` when it is out of range or not 4-aligned.
    fn load_i32(&self, off: usize) -> Option<i32>;

    /// Copies `dst.len()` bytes from `off` into `dst`; `false` (and `dst` untouched) when the
    /// range is out of bounds.
    fn copy_out(&self, off: usize, dst: &mut [u8]) -> bool;
}

/// A view over a plain slice: an `.ibt` image or a synthetic live image.
#[derive(Clone, Copy, Debug)]
pub struct SliceView<'a>(&'a [u8]);

impl<'a> SliceView<'a> {
    /// A view over `bytes`.
    pub fn new(bytes: &'a [u8]) -> SliceView<'a> {
        SliceView(bytes)
    }
}

impl View for SliceView<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn load_i32(&self, off: usize) -> Option<i32> {
        if !off.is_multiple_of(4) {
            return None;
        }
        let end = off.checked_add(4)?;
        let raw: [u8; 4] = self.0.get(off..end)?.try_into().ok()?;
        Some(i32::from_le_bytes(raw))
    }

    fn copy_out(&self, off: usize, dst: &mut [u8]) -> bool {
        let Some(end) = off.checked_add(dst.len()) else {
            return false;
        };
        match self.0.get(off..end) {
            Some(src) => {
                dst.copy_from_slice(src);
                true
            }
            None => false,
        }
    }
}

impl<V: View + ?Sized> View for &V {
    fn len(&self) -> usize {
        (**self).len()
    }

    fn load_i32(&self, off: usize) -> Option<i32> {
        (**self).load_i32(off)
    }

    fn copy_out(&self, off: usize, dst: &mut [u8]) -> bool {
        (**self).copy_out(off, dst)
    }
}

impl<V: View + ?Sized> View for std::sync::Arc<V> {
    fn len(&self) -> usize {
        (**self).len()
    }

    fn load_i32(&self, off: usize) -> Option<i32> {
        (**self).load_i32(off)
    }

    fn copy_out(&self, off: usize, dst: &mut [u8]) -> bool {
        (**self).copy_out(off, dst)
    }
}
