use arkhe_abi::{Error, Handle};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rights(u8);

impl Rights {
    pub const SEND: Self = Self(1);
    pub const RECEIVE: Self = Self(2);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectId(pub u64);

#[derive(Clone, Copy)]
struct Entry {
    object: ObjectId,
    rights: Rights,
}

#[derive(Clone, Copy)]
struct Slot {
    generation: u32,
    entry: Option<Entry>,
}

pub struct CapabilityTable<const N: usize> {
    slots: [Slot; N],
}

impl<const N: usize> Default for CapabilityTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> CapabilityTable<N> {
    pub const fn new() -> Self {
        Self {
            slots: [Slot {
                generation: 1,
                entry: None,
            }; N],
        }
    }

    pub fn insert(&mut self, object: ObjectId, rights: Rights) -> Result<Handle, Error> {
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.entry.is_none() && slot.generation != 0 {
                let index = u32::try_from(index).map_err(|_| Error::NoSpace)?;
                slot.entry = Some(Entry { object, rights });
                return Ok(Handle::from_raw(
                    (u64::from(slot.generation) << 32) | u64::from(index),
                ));
            }
        }
        Err(Error::NoSpace)
    }

    pub fn resolve(&self, handle: Handle, required: Rights) -> Result<ObjectId, Error> {
        let slot = self.slot(handle)?;
        let entry = slot.entry.ok_or(Error::InvalidHandle)?;
        if !entry.rights.contains(required) {
            return Err(Error::PermissionDenied);
        }
        Ok(entry.object)
    }

    pub fn close(&mut self, handle: Handle) -> Result<(), Error> {
        self.slot(handle)?;
        let slot = &mut self.slots[handle.raw() as u32 as usize];
        slot.entry = None;
        // Exhausted generations retire the slot so stale handles never become valid again.
        slot.generation = slot.generation.checked_add(1).unwrap_or(0);
        Ok(())
    }

    fn slot(&self, handle: Handle) -> Result<&Slot, Error> {
        let index = handle.raw() as u32 as usize;
        let generation = (handle.raw() >> 32) as u32;
        let slot = self.slots.get(index).ok_or(Error::InvalidHandle)?;
        if generation == 0 || generation != slot.generation || slot.entry.is_none() {
            return Err(Error::InvalidHandle);
        }
        Ok(slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rights_are_checked_and_stale_handles_rejected() {
        let mut table = CapabilityTable::<1>::new();
        let old = table.insert(ObjectId(4), Rights::SEND).unwrap();
        assert_eq!(
            table.resolve(old, Rights::RECEIVE),
            Err(Error::PermissionDenied)
        );
        assert_eq!(table.resolve(old, Rights::SEND), Ok(ObjectId(4)));
        table.close(old).unwrap();
        let new = table.insert(ObjectId(8), Rights::RECEIVE).unwrap();
        assert_eq!(table.resolve(old, Rights::SEND), Err(Error::InvalidHandle));
        assert_eq!(table.close(old), Err(Error::InvalidHandle));
        assert_eq!(table.resolve(new, Rights::RECEIVE), Ok(ObjectId(8)));
    }

    #[test]
    fn exhausted_generation_is_never_reused() {
        let mut table = CapabilityTable::<1>::new();
        table.slots[0].generation = u32::MAX;
        let handle = table.insert(ObjectId(1), Rights::SEND).unwrap();
        table.close(handle).unwrap();
        assert_eq!(table.insert(ObjectId(2), Rights::SEND), Err(Error::NoSpace));
    }

    #[test]
    fn rejects_forged_invalid_indices_and_full_tables() {
        let mut table = CapabilityTable::<1>::new();
        let handle = table
            .insert(ObjectId(1), Rights::SEND.union(Rights::RECEIVE))
            .unwrap();
        assert_eq!(
            table.resolve(handle, Rights::SEND.union(Rights::RECEIVE)),
            Ok(ObjectId(1))
        );
        assert_eq!(table.insert(ObjectId(2), Rights::SEND), Err(Error::NoSpace));
        for raw in [0, u64::MAX, 1 << 33, (1 << 32) | 100] {
            assert_eq!(
                table.resolve(Handle::from_raw(raw), Rights::SEND),
                Err(Error::InvalidHandle)
            );
        }
        assert_eq!(
            CapabilityTable::<0>::new().insert(ObjectId(1), Rights::SEND),
            Err(Error::NoSpace)
        );
    }
}
