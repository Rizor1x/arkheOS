#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduleError {
    InvalidSlot,
    Occupied,
    InvalidState,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum State {
    Vacant,
    Runnable,
    Blocked,
}

/// Single-CPU round robin. Blocking preserves a live slot until wake or removal.
pub struct Scheduler<const N: usize> {
    slots: [State; N],
    current: Option<usize>,
}
impl<const N: usize> Scheduler<N> {
    pub const fn new() -> Self {
        Self {
            slots: [State::Vacant; N],
            current: None,
        }
    }
    pub fn insert(&mut self, slot: usize) -> Result<(), ScheduleError> {
        let state = self.slots.get_mut(slot).ok_or(ScheduleError::InvalidSlot)?;
        if *state != State::Vacant {
            return Err(ScheduleError::Occupied);
        }
        *state = State::Runnable;
        Ok(())
    }
    pub fn vacant_slot(&self) -> Option<usize> {
        self.slots.iter().position(|s| *s == State::Vacant)
    }
    pub fn current(&self) -> Option<usize> {
        self.current
    }
    pub fn live_count(&self) -> usize {
        self.slots.iter().filter(|&&s| s != State::Vacant).count()
    }
    pub fn advance(&mut self) -> Option<usize> {
        let start = self.current.map_or(0, |slot| slot + 1);
        self.current = (start..N)
            .chain(0..start)
            .find(|&slot| self.slots[slot] == State::Runnable);
        self.current
    }
    pub fn block_current(&mut self) -> Option<usize> {
        self.transition(State::Blocked)
    }
    pub fn remove_current(&mut self) -> Option<usize> {
        self.transition(State::Vacant)
    }
    pub fn wake(&mut self, slot: usize) -> Result<(), ScheduleError> {
        let state = self.slots.get_mut(slot).ok_or(ScheduleError::InvalidSlot)?;
        if *state != State::Blocked {
            return Err(ScheduleError::InvalidState);
        }
        *state = State::Runnable;
        Ok(())
    }
    fn transition(&mut self, state: State) -> Option<usize> {
        let slot = self.current?;
        self.slots[slot] = state;
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
        assert_eq!(scheduler.vacant_slot(), Some(0));
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
        assert_eq!(empty.vacant_slot(), None);
        assert_eq!(empty.insert(0), Err(ScheduleError::InvalidSlot));
    }
    #[test]
    fn blocked_tasks_are_live_but_never_selected_without_wake() {
        let mut scheduler = Scheduler::<2>::new();
        scheduler.insert(0).unwrap();
        scheduler.insert(1).unwrap();
        assert_eq!(scheduler.vacant_slot(), None);
        assert_eq!(scheduler.advance(), Some(0));
        assert_eq!(scheduler.block_current(), Some(0));
        assert_eq!(scheduler.current(), Some(1));
        assert_eq!(scheduler.advance(), Some(1));
        assert_eq!(scheduler.block_current(), Some(1));
        assert_eq!(scheduler.advance(), None);
        assert_eq!(scheduler.live_count(), 2);
        assert_eq!(scheduler.vacant_slot(), None);
        assert_eq!(scheduler.insert(0), Err(ScheduleError::Occupied));
        scheduler.wake(0).unwrap();
        assert_eq!(scheduler.wake(0), Err(ScheduleError::InvalidState));
        assert_eq!(scheduler.advance(), Some(0));
        scheduler.remove_current();
        assert_eq!(scheduler.wake(0), Err(ScheduleError::InvalidState));
        assert_eq!(scheduler.live_count(), 1);
        assert_eq!(scheduler.wake(2), Err(ScheduleError::InvalidSlot));
        scheduler.wake(1).unwrap();
        assert_eq!(scheduler.advance(), Some(1));
    }
}
