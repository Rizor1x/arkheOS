use arkhe_abi::{PRIVATE_HEAP_BASE, PRIVATE_HEAP_MAX_PAGES, USER_PAGE_BYTES};

/// The target size is validated before any mapping or allocation changes.
#[derive(Debug, Eq, PartialEq)]
pub struct Resize {
    pub old: usize,
    pub new: usize,
}

impl Resize {
    pub fn new(old: usize, requested: u64) -> Option<Self> {
        let new = usize::try_from(requested).ok()?;
        if old > PRIVATE_HEAP_MAX_PAGES || new > PRIVATE_HEAP_MAX_PAGES {
            return None;
        }
        Some(Self { old, new })
    }
}

pub fn page_address(index: usize) -> Option<u64> {
    if index >= PRIVATE_HEAP_MAX_PAGES {
        return None;
    }
    PRIVATE_HEAP_BASE.checked_add(u64::try_from(index.checked_mul(USER_PAGE_BYTES)?).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_resize_without_truncation_and_allows_empty_and_unchanged() {
        for (old, new) in [(0, 0), (0, 64), (64, 0), (64, 64), (1, 2), (2, 1)] {
            let resize = Resize::new(old, new).unwrap();
            assert_eq!((resize.old, resize.new), (old, new as usize));
        }
        for requested in [65, 1 << 32, u64::MAX] {
            assert!(Resize::new(1, requested).is_none());
        }
        assert!(Resize::new(65, 0).is_none());
        assert!(Resize::new(usize::MAX, 0).is_none());
    }

    #[test]
    fn bounds_heap_addresses_away_from_guard_pages() {
        assert_eq!(page_address(0), Some(PRIVATE_HEAP_BASE));
        assert_eq!(page_address(63), Some(PRIVATE_HEAP_BASE + 63 * 4096));
        assert_eq!(page_address(64), None);
        assert_eq!(page_address(usize::MAX), None);
        let lower = PRIVATE_HEAP_BASE - USER_PAGE_BYTES as u64;
        let upper = PRIVATE_HEAP_BASE + (PRIVATE_HEAP_MAX_PAGES * USER_PAGE_BYTES) as u64;
        assert_eq!(
            lower >> 21,
            upper >> 21,
            "heap and guards require one leaf table"
        );
    }
}
