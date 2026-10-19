use core::alloc::Layout;

/// An arena lends disjoint buffers for the lifetime of its borrowed backing memory.
/// Individual buffers are reclaimed together when that backing allocation is released.
pub struct Arena<'a> {
    remaining: &'a mut [u8],
}

impl<'a> Arena<'a> {
    pub fn new(bytes: &'a mut [u8]) -> Self {
        Self { remaining: bytes }
    }

    pub fn allocate(&mut self, layout: Layout) -> Option<&'a mut [u8]> {
        if layout.size() == 0 {
            return None;
        }
        let start = self.remaining.as_ptr() as usize;
        let aligned = start.checked_add(layout.align() - 1)? & !(layout.align() - 1);
        let padding = aligned.checked_sub(start)?;
        let end = padding.checked_add(layout.size())?;
        if end > self.remaining.len() {
            return None;
        }
        let (prefix, rest) = core::mem::take(&mut self.remaining).split_at_mut(end);
        self.remaining = rest;
        Some(&mut prefix[padding..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lends_disjoint_aligned_buffers_and_preserves_space_on_failure() {
        let mut backing = [0; 1024];
        let mut arena = Arena::new(&mut backing[1..]);
        let a = arena
            .allocate(Layout::from_size_align(17, 64).unwrap())
            .unwrap();
        a.fill(0x31);
        let b = arena
            .allocate(Layout::from_size_align(19, 256).unwrap())
            .unwrap();
        b.fill(0x72);
        assert_eq!(a.as_ptr() as usize % 64, 0);
        assert_eq!(b.as_ptr() as usize % 256, 0);
        assert_eq!(a, &[0x31; 17]);
        let left = arena.remaining.len();
        assert!(
            arena
                .allocate(Layout::from_size_align(0, 1).unwrap())
                .is_none()
        );
        assert!(
            arena
                .allocate(Layout::from_size_align(isize::MAX as usize, 1).unwrap())
                .is_none()
        );
        assert_eq!(arena.remaining.len(), left);
        assert_eq!(
            arena
                .allocate(Layout::from_size_align(left, 1).unwrap())
                .unwrap()
                .len(),
            left
        );
        assert!(arena.allocate(Layout::new::<u8>()).is_none());
    }
}
