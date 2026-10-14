use crate::memory::{FrameAllocator, FrameRange, PAGE_SIZE, RegionError};

const MAX_RANGES: usize = 128;
const EMPTY_RANGE: FrameRange = FrameRange::EMPTY;

#[derive(Debug, Eq, PartialEq)]
pub enum PoolError {
    InvalidRegions(RegionError),
    TooManyRegions,
    AlreadyInitialized,
    InvalidFrame,
}

/// An owned allocation. Moving it back into its issuing pool releases the frame.
#[derive(Debug)]
pub struct Frame {
    physical: u64,
    slot: usize,
    generation: u64,
}
impl Frame {
    pub fn physical(&self) -> u64 {
        self.physical
    }
}

#[derive(Clone, Copy)]
struct Slot {
    physical: u64,
    live: bool,
    generation: u64,
}
const EMPTY_SLOT: Slot = Slot {
    physical: 0,
    live: false,
    generation: 0,
};

/// Bounded frame ownership; never allocates reserved RAM or wraps a lease generation.
pub struct FramePool<const N: usize> {
    ranges: [FrameRange; MAX_RANGES],
    count: usize,
    region: usize,
    next: u64,
    slots: [Slot; N],
    live: usize,
    initialized: bool,
}

impl<const N: usize> FramePool<N> {
    pub const fn new() -> Self {
        Self {
            ranges: [EMPTY_RANGE; MAX_RANGES],
            count: 0,
            region: 0,
            next: 0,
            slots: [EMPTY_SLOT; N],
            live: 0,
            initialized: false,
        }
    }

    /// Ranges must refer to RAM exclusively owned by this allocator.
    pub fn initialize(&mut self, ranges: &[FrameRange]) -> Result<(), PoolError> {
        if self.initialized {
            return Err(PoolError::AlreadyInitialized);
        }
        if ranges.len() > MAX_RANGES {
            return Err(PoolError::TooManyRegions);
        }
        FrameAllocator::new(ranges).map_err(PoolError::InvalidRegions)?;
        self.ranges[..ranges.len()].copy_from_slice(ranges);
        self.count = ranges.len();
        self.initialized = true;
        Ok(())
    }

    pub fn allocate(&mut self) -> Option<Frame> {
        if !self.initialized {
            return None;
        }
        let slot = self
            .slots
            .iter()
            .position(|slot| !slot.live && slot.generation != u64::MAX)?;
        let physical = if self.slots[slot].generation != 0 {
            self.slots[slot].physical
        } else {
            self.next_physical()?
        };
        let generation = self.slots[slot].generation + 1;
        self.slots[slot] = Slot {
            physical,
            live: true,
            generation,
        };
        self.live += 1;
        Some(Frame {
            physical,
            slot,
            generation,
        })
    }

    pub fn release(&mut self, frame: Frame) -> Result<(), PoolError> {
        let slot = self
            .slots
            .get_mut(frame.slot)
            .ok_or(PoolError::InvalidFrame)?;
        if !slot.live || slot.physical != frame.physical || slot.generation != frame.generation {
            return Err(PoolError::InvalidFrame);
        }
        slot.live = false;
        self.live -= 1;
        Ok(())
    }

    pub fn live_count(&self) -> usize {
        self.live
    }

    fn next_physical(&mut self) -> Option<u64> {
        while let Some(range) = self.ranges[..self.count].get(self.region) {
            self.next = self.next.max(range.start());
            if self.next < range.end() {
                let physical = self.next;
                self.next += PAGE_SIZE;
                if physical != 0 {
                    return Some(physical);
                }
            } else {
                self.region += 1;
                self.next = 0;
            }
        }
        None
    }
}

impl<const N: usize> Default for FramePool<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reuses_frames_and_rejects_stale_or_duplicate_releases() {
        let mut pool = FramePool::<2>::new();
        assert!(pool.allocate().is_none());
        pool.initialize(&[FrameRange::from_region(0, 3 * PAGE_SIZE).unwrap()])
            .unwrap();
        let a = pool.allocate().unwrap();
        let stale = Frame {
            physical: a.physical,
            slot: a.slot,
            generation: a.generation,
        };
        assert_eq!(a.physical(), PAGE_SIZE);
        let b = pool.allocate().unwrap();
        assert_eq!(b.physical(), 2 * PAGE_SIZE);
        assert!(pool.allocate().is_none());
        pool.release(a).unwrap();
        let a = pool.allocate().unwrap();
        assert_eq!(a.physical(), PAGE_SIZE);
        assert_eq!(pool.release(stale), Err(PoolError::InvalidFrame));
        let duplicate = Frame {
            physical: a.physical,
            slot: a.slot,
            generation: a.generation,
        };
        pool.release(a).unwrap();
        assert_eq!(pool.release(duplicate), Err(PoolError::InvalidFrame));
        pool.release(b).unwrap();
        assert_eq!(pool.live_count(), 0);
    }

    #[test]
    fn validates_ranges_and_handles_exhaustion_without_losing_slots() {
        let mut pool = FramePool::<4>::new();
        let range = FrameRange::from_region(PAGE_SIZE, PAGE_SIZE).unwrap();
        assert_eq!(
            pool.initialize(&[range, range]),
            Err(PoolError::InvalidRegions(RegionError::OverlapOrUnsorted))
        );
        pool.initialize(&[range]).unwrap();
        assert_eq!(pool.initialize(&[]), Err(PoolError::AlreadyInitialized));
        let frame = pool.allocate().unwrap();
        assert!(pool.allocate().is_none());
        pool.release(frame).unwrap();
        assert_eq!(pool.allocate().unwrap().physical(), PAGE_SIZE);
        let mut empty = FramePool::<0>::new();
        empty.initialize(&[range]).unwrap();
        assert!(empty.allocate().is_none());
    }

    #[test]
    fn retires_generation_before_wrap_and_bounds_region_storage() {
        let mut pool = FramePool::<1>::new();
        assert_eq!(
            pool.initialize(&[EMPTY_RANGE; 129]),
            Err(PoolError::TooManyRegions)
        );
        pool.initialize(&[FrameRange::from_region(PAGE_SIZE, PAGE_SIZE).unwrap()])
            .unwrap();
        pool.slots[0].generation = u64::MAX;
        assert!(pool.allocate().is_none());
    }
}
