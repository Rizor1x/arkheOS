use arkhe_abi::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompletionId {
    slot: usize,
    generation: u64,
}
#[derive(Clone, Copy)]
struct Record {
    generation: u64,
    live: bool,
    watched: bool,
    outcome: Option<u64>,
}
const EMPTY: Record = Record {
    generation: 1,
    live: false,
    watched: false,
    outcome: None,
};

/// Exit records outlive execution slots. Dropping a watcher never kills its child.
pub struct Completions<const N: usize> {
    records: [Record; N],
}
impl<const N: usize> Completions<N> {
    pub const fn new() -> Self {
        Self {
            records: [EMPTY; N],
        }
    }
    /// Reserve one running child and its unique watcher.
    pub fn create(&mut self) -> Result<CompletionId, Error> {
        let (index, record) = self
            .records
            .iter_mut()
            .enumerate()
            .find(|(_, r)| !r.live && r.generation != 0)
            .ok_or(Error::NoSpace)?;
        record.live = true;
        record.watched = true;
        record.outcome = None;
        Ok(CompletionId {
            slot: index,
            generation: record.generation,
        })
    }
    /// Observe without consuming; a running child returns `WouldBlock`.
    pub fn status(&self, id: CompletionId) -> Result<u64, Error> {
        let record = self.records.get(id.slot).ok_or(Error::InvalidHandle)?;
        if !record.live || record.generation != id.generation || !record.watched {
            return Err(Error::InvalidHandle);
        }
        record.outcome.ok_or(Error::WouldBlock)
    }
    /// Publish once, releasing unwatched records immediately.
    pub fn complete(&mut self, id: CompletionId, outcome: u64) -> Result<(), Error> {
        let record = self.get(id)?;
        if record.outcome.is_some() {
            return Err(Error::InvalidHandle);
        }
        record.outcome = Some(outcome);
        if !record.watched {
            Self::retire(record);
        }
        Ok(())
    }
    /// Consume a completed record. A running record stays unchanged.
    pub fn reap(&mut self, id: CompletionId) -> Result<u64, Error> {
        let outcome = self.status(id)?;
        Self::retire(self.get(id)?);
        Ok(outcome)
    }
    /// Drop the watcher, retaining a running record until its child completes.
    pub fn detach(&mut self, id: CompletionId) -> Result<(), Error> {
        let record = self.get(id)?;
        if !record.watched {
            return Err(Error::InvalidHandle);
        }
        record.watched = false;
        if record.outcome.is_some() {
            Self::retire(record);
        }
        Ok(())
    }
    pub fn live_count(&self) -> usize {
        self.records.iter().filter(|r| r.live).count()
    }
    fn get(&mut self, id: CompletionId) -> Result<&mut Record, Error> {
        let record = self.records.get_mut(id.slot).ok_or(Error::InvalidHandle)?;
        if !record.live || record.generation != id.generation {
            return Err(Error::InvalidHandle);
        }
        Ok(record)
    }
    fn retire(record: &mut Record) {
        record.live = false;
        record.watched = false;
        record.outcome = None;
        record.generation = record.generation.checked_add(1).unwrap_or(0);
    }
}
impl<const N: usize> Default for Completions<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retains_status_until_reap_and_rejects_stale_ids() {
        let mut table = Completions::<1>::new();
        let id = table.create().unwrap();
        assert_eq!(table.status(id), Err(Error::WouldBlock));
        assert_eq!(table.reap(id), Err(Error::WouldBlock));
        table.complete(id, 7).unwrap();
        assert_eq!(table.create(), Err(Error::NoSpace));
        assert_eq!(table.status(id), Ok(7));
        assert_eq!(table.complete(id, 9), Err(Error::InvalidHandle));
        assert_eq!(table.reap(id), Ok(7));
        let next = table.create().unwrap();
        assert_ne!(id, next);
        assert_eq!(table.status(id), Err(Error::InvalidHandle));
        assert_eq!(table.detach(id), Err(Error::InvalidHandle));
        assert_eq!(table.status(next), Err(Error::WouldBlock));
    }
    #[test]
    fn detaches_live_children_and_releases_unobserved_completions() {
        let mut table = Completions::<2>::new();
        let a = table.create().unwrap();
        let b = table.create().unwrap();
        table.detach(a).unwrap();
        assert_eq!(table.detach(a), Err(Error::InvalidHandle));
        assert_eq!(table.status(a), Err(Error::InvalidHandle));
        assert_eq!(table.live_count(), 2);
        table.complete(a, 99).unwrap();
        table.complete(b, 0x1_0000_0006).unwrap();
        table.detach(b).unwrap();
        assert_eq!(table.live_count(), 0);
        assert_eq!(table.complete(a, 0), Err(Error::InvalidHandle));
    }
    #[test]
    fn retires_exhausted_generations_and_handles_zero_quota() {
        let mut table = Completions::<1>::new();
        table.records[0].generation = u64::MAX;
        let id = table.create().unwrap();
        table.complete(id, 0).unwrap();
        table.reap(id).unwrap();
        assert_eq!(table.create(), Err(Error::NoSpace));
        assert_eq!(Completions::<0>::new().create(), Err(Error::NoSpace));
    }
}
