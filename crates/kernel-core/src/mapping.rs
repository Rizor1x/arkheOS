use crate::memory::PAGE_SIZE;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MappingError {
    InvalidRange,
    WritableCode,
    VirtualOverlap,
    PhysicalAlias,
    NoSpace,
}

/// One page-aligned, canonical supervisor mapping. Physical aliases are excluded
/// from a boot plan so writable direct maps cannot bypass executable permissions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mapping {
    virtual_start: u64,
    physical_start: u64,
    length: u64,
    writable: bool,
    executable: bool,
}

impl Mapping {
    pub fn new(
        virtual_start: u64,
        physical_start: u64,
        length: u64,
        writable: bool,
        executable: bool,
    ) -> Result<Self, MappingError> {
        if writable && executable {
            return Err(MappingError::WritableCode);
        }
        let virtual_end = virtual_start
            .checked_add(length)
            .ok_or(MappingError::InvalidRange)?;
        let physical_end = physical_start
            .checked_add(length)
            .ok_or(MappingError::InvalidRange)?;
        if length == 0
            || !virtual_start.is_multiple_of(PAGE_SIZE)
            || !physical_start.is_multiple_of(PAGE_SIZE)
            || !length.is_multiple_of(PAGE_SIZE)
            || virtual_start >> 47 != 0x1ffff
            || (virtual_end - 1) >> 47 != 0x1ffff
            || physical_end > 1 << 52
        {
            return Err(MappingError::InvalidRange);
        }
        Ok(Self {
            virtual_start,
            physical_start,
            length,
            writable,
            executable,
        })
    }

    pub fn virtual_start(self) -> u64 {
        self.virtual_start
    }
    pub fn physical_start(self) -> u64 {
        self.physical_start
    }
    pub fn length(self) -> u64 {
        self.length
    }
    pub fn writable(self) -> bool {
        self.writable
    }
    pub fn executable(self) -> bool {
        self.executable
    }
}

pub struct MappingPlan<const N: usize> {
    entries: [Option<Mapping>; N],
    count: usize,
}

impl<const N: usize> MappingPlan<N> {
    pub const fn new() -> Self {
        Self {
            entries: [None; N],
            count: 0,
        }
    }

    /// Reject conflicts before publishing an entry; failure leaves the plan intact.
    pub fn insert(&mut self, mapping: Mapping) -> Result<(), MappingError> {
        for previous in self.entries() {
            if overlaps(
                mapping.virtual_start,
                mapping.length,
                previous.virtual_start,
                previous.length,
            ) {
                return Err(MappingError::VirtualOverlap);
            }
            if overlaps(
                mapping.physical_start,
                mapping.length,
                previous.physical_start,
                previous.length,
            ) {
                return Err(MappingError::PhysicalAlias);
            }
        }
        let slot = self
            .entries
            .get_mut(self.count)
            .ok_or(MappingError::NoSpace)?;
        *slot = Some(mapping);
        self.count += 1;
        Ok(())
    }

    pub fn entries(&self) -> impl Iterator<Item = Mapping> + '_ {
        self.entries[..self.count].iter().flatten().copied()
    }
}

impl<const N: usize> Default for MappingPlan<N> {
    fn default() -> Self {
        Self::new()
    }
}

fn overlaps(a: u64, a_length: u64, b: u64, b_length: u64) -> bool {
    a < b + b_length && b < a + a_length
}

#[cfg(test)]
mod tests {
    use super::*;
    const BASE: u64 = 0xffff_ffff_8000_0000;

    #[test]
    fn rejects_invalid_ranges_and_writable_executable_pages() {
        for (virtual_start, physical_start, length) in [
            (BASE, 0x1000, 0),
            (BASE + 1, 0x1000, PAGE_SIZE),
            (BASE, 1, PAGE_SIZE),
            (BASE, 0x1000, 1),
            (0x8000_0000_0000, 0x1000, PAGE_SIZE),
            (0x10000, 0x1000, PAGE_SIZE),
            (u64::MAX - 4095, 0x1000, PAGE_SIZE),
            (BASE, u64::MAX - 4095, PAGE_SIZE),
            (BASE, 1 << 52, PAGE_SIZE),
        ] {
            assert_eq!(
                Mapping::new(virtual_start, physical_start, length, false, false),
                Err(MappingError::InvalidRange)
            );
        }
        assert_eq!(
            Mapping::new(BASE, 0x1000, PAGE_SIZE, true, true),
            Err(MappingError::WritableCode)
        );
        assert!(Mapping::new(BASE, (1 << 52) - PAGE_SIZE, PAGE_SIZE, false, true).is_ok());
    }

    #[test]
    fn conflicts_and_capacity_preserve_the_published_plan() {
        let code = Mapping::new(BASE, 0x1000, PAGE_SIZE * 2, false, true).unwrap();
        let mut plan = MappingPlan::<2>::new();
        plan.insert(code).unwrap();
        assert_eq!(
            plan.insert(Mapping::new(BASE + PAGE_SIZE, 0x5000, PAGE_SIZE, true, false).unwrap()),
            Err(MappingError::VirtualOverlap)
        );
        assert_eq!(
            plan.insert(Mapping::new(BASE + 0x10000, 0x2000, PAGE_SIZE, true, false).unwrap()),
            Err(MappingError::PhysicalAlias)
        );
        {
            let mut entries = plan.entries();
            assert_eq!(entries.next(), Some(code));
            assert_eq!(entries.next(), None);
        }
        plan.insert(Mapping::new(BASE + PAGE_SIZE * 2, 0x3000, PAGE_SIZE, true, false).unwrap())
            .unwrap();
        assert_eq!(
            plan.insert(Mapping::new(BASE + 0x10000, 0x9000, PAGE_SIZE, false, false).unwrap()),
            Err(MappingError::NoSpace)
        );
        assert_eq!(plan.entries().count(), 2);
        assert_eq!(
            MappingPlan::<0>::new().insert(code),
            Err(MappingError::NoSpace)
        );
    }
}
