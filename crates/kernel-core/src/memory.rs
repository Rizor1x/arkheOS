pub const PAGE_SIZE: u64 = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegionError {
    Overflow,
    OverlapOrUnsorted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameRange {
    start: u64,
    end: u64,
}

impl FrameRange {
    pub const EMPTY: Self = Self { start: 0, end: 0 };
    pub const fn start(self) -> u64 {
        self.start
    }
    pub const fn end(self) -> u64 {
        self.end
    }

    pub fn from_region(base: u64, length: u64) -> Result<Self, RegionError> {
        let end = base.checked_add(length).ok_or(RegionError::Overflow)?;
        let Some(start) = base.checked_add(PAGE_SIZE - 1) else {
            return Ok(Self { start: 0, end: 0 });
        };
        let start = start & !(PAGE_SIZE - 1);
        let end = end & !(PAGE_SIZE - 1);
        Ok(Self {
            start: start.min(end),
            end,
        })
    }

    pub const fn count(self) -> u64 {
        (self.end - self.start) / PAGE_SIZE
    }
}

pub struct FrameAllocator<'a> {
    ranges: &'a [FrameRange],
    region: usize,
    next: u64,
}

impl<'a> FrameAllocator<'a> {
    /// Ranges must contain only exclusively owned, bootloader-reported usable RAM.
    pub fn new(ranges: &'a [FrameRange]) -> Result<Self, RegionError> {
        let mut end = 0;
        for range in ranges.iter().filter(|range| range.count() != 0) {
            if range.start < end {
                return Err(RegionError::OverlapOrUnsorted);
            }
            end = range.end;
        }
        Ok(Self {
            ranges,
            region: 0,
            next: 0,
        })
    }

    pub fn allocate(&mut self) -> Option<u64> {
        while let Some(range) = self.ranges.get(self.region) {
            self.next = self.next.max(range.start);
            if self.next < range.end {
                let frame = self.next;
                self.next += PAGE_SIZE;
                return Some(frame);
            }
            self.region += 1;
            self.next = 0;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_only_whole_frames_in_order() {
        let ranges = [
            FrameRange::from_region(1, 16383).unwrap(),
            FrameRange::from_region(0x10000, 4096).unwrap(),
        ];
        let mut allocator = FrameAllocator::new(&ranges).unwrap();
        assert_eq!(allocator.allocate(), Some(4096));
        assert_eq!(allocator.allocate(), Some(8192));
        assert_eq!(allocator.allocate(), Some(12288));
        assert_eq!(allocator.allocate(), Some(0x10000));
        assert_eq!(allocator.allocate(), None);
        assert_eq!(allocator.allocate(), None);
    }

    #[test]
    fn handles_empty_short_and_overflowing_regions() {
        assert_eq!(FrameRange::from_region(3, 0).unwrap().count(), 0);
        assert_eq!(FrameRange::from_region(3, 4096).unwrap().count(), 0);
        assert_eq!(
            FrameRange::from_region(u64::MAX, 1),
            Err(RegionError::Overflow)
        );
        assert_eq!(FrameRange::from_region(u64::MAX - 1, 1).unwrap().count(), 0);
        assert_eq!(FrameAllocator::new(&[]).unwrap().allocate(), None);
    }

    #[test]
    fn rejects_overlapping_and_unsorted_ranges() {
        let a = FrameRange::from_region(4096, 8192).unwrap();
        let b = FrameRange::from_region(8192, 8192).unwrap();
        assert!(matches!(
            FrameAllocator::new(&[a, b]),
            Err(RegionError::OverlapOrUnsorted)
        ));
        assert!(matches!(
            FrameAllocator::new(&[b, a]),
            Err(RegionError::OverlapOrUnsorted)
        ));
    }

    #[test]
    fn skips_empty_ranges_between_usable_ranges() {
        let ranges = [
            FrameRange::from_region(4096, 4096).unwrap(),
            FrameRange::from_region(3, 1).unwrap(),
            FrameRange::from_region(8192, 4096).unwrap(),
        ];
        let mut allocator = FrameAllocator::new(&ranges).unwrap();
        assert_eq!(allocator.allocate(), Some(4096));
        assert_eq!(allocator.allocate(), Some(8192));
        assert_eq!(allocator.allocate(), None);
    }
}
