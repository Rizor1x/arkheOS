#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduleError {
    InvalidSlot,
    Occupied,
}

/// Single-CPU round robin. A slot remains runnable until explicitly removed.
pub struct Scheduler<const N: usize> {
    runnable: [bool; N],
    current: Option<usize>,
}

impl<const N: usize> Scheduler<N> {
    pub const fn new() -> Self {
        Self {
            runnable: [false; N],
            current: None,
        }
    }

    pub fn insert(&mut self, slot: usize) -> Result<(), ScheduleError> {
        let runnable = self
            .runnable
            .get_mut(slot)
            .ok_or(ScheduleError::InvalidSlot)?;
        if *runnable {
            return Err(ScheduleError::Occupied);
        }
        *runnable = true;
        Ok(())
    }

    pub fn current(&self) -> Option<usize> {
        self.current
    }

    pub fn advance(&mut self) -> Option<usize> {
        let start = self.current.map_or(0, |slot| slot + 1);
        self.current = (start..N).chain(0..start).find(|&slot| self.runnable[slot]);
        self.current
    }

    pub fn remove_current(&mut self) -> Option<usize> {
        let slot = self.current?;
        self.runnable[slot] = false;
        self.advance();
        Some(slot)
    }
}

impl<const N: usize> Default for Scheduler<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_fairly_and_skips_empty_slots() {
        let mut scheduler = Scheduler::<4>::new();
        scheduler.insert(0).unwrap();
        scheduler.insert(2).unwrap();
        for expected in [0, 2, 0, 2, 0, 2] {
            assert_eq!(scheduler.advance(), Some(expected));
        }
        assert_eq!(scheduler.remove_current(), Some(2));
        assert_eq!(scheduler.current(), Some(0));
        assert_eq!(scheduler.advance(), Some(0));
        assert_eq!(scheduler.remove_current(), Some(0));
        assert_eq!(scheduler.current(), None);
        assert_eq!(scheduler.advance(), None);
    }

    #[test]
    fn rejects_invalid_transitions_and_allows_explicit_slot_reuse() {
        let mut scheduler = Scheduler::<2>::new();
        assert_eq!(scheduler.remove_current(), None);
        assert_eq!(scheduler.insert(2), Err(ScheduleError::InvalidSlot));
        scheduler.insert(1).unwrap();
        assert_eq!(scheduler.insert(1), Err(ScheduleError::Occupied));
        assert_eq!(scheduler.advance(), Some(1));
        scheduler.remove_current();
        assert_eq!(scheduler.advance(), None);
        scheduler.insert(1).unwrap();
        assert_eq!(scheduler.advance(), Some(1));
        let mut empty = Scheduler::<0>::new();
        assert_eq!(empty.advance(), None);
        assert_eq!(empty.insert(0), Err(ScheduleError::InvalidSlot));
    }
}
