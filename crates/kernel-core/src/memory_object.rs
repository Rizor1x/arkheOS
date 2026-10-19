use crate::frame_pool::{Frame, FramePool};
use arkhe_abi::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryId {
    slot: usize,
    generation: u64,
}

/// One retained mapping, independent of capability references. It must be returned
/// only after its hardware translations have been removed and invalidated.
pub struct MappingLease {
    id: MemoryId,
}

struct Object<const P: usize> {
    frames: [Option<Frame>; P],
    pages: usize,
    caps: u16,
    mappings: u16,
}

struct Slot<const P: usize> {
    generation: u64,
    object: Option<Object<P>>,
}

/// Owns frames until the last capability and mapping are released. No reference
/// to frame contents escapes this registry; callers mediate hardware access.
pub struct MemoryObjects<const N: usize, const P: usize> {
    slots: [Slot<P>; N],
}

impl<const N: usize, const P: usize> Default for MemoryObjects<N, P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize, const P: usize> MemoryObjects<N, P> {
    pub const fn new() -> Self {
        Self {
            slots: [const {
                Slot {
                    generation: 1,
                    object: None,
                }
            }; N],
        }
    }

    /// Allocate and initialize every page before publishing the first reference.
    /// An error returns all pending frames without consuming an object generation.
    pub fn create<const F: usize>(
        &mut self,
        pool: &mut FramePool<F>,
        pages: u64,
        mut initialize: impl FnMut(u64) -> Result<(), Error>,
    ) -> Result<MemoryId, Error> {
        let pages = usize::try_from(pages).map_err(|_| Error::InvalidArgument)?;
        if pages == 0 || pages > P {
            return Err(Error::InvalidArgument);
        }
        let index = self
            .slots
            .iter()
            .position(|s| s.object.is_none() && s.generation != 0)
            .ok_or(Error::NoSpace)?;
        let mut object = Object {
            frames: [const { None }; P],
            pages,
            caps: 1,
            mappings: 0,
        };
        for page in 0..pages {
            let result = pool.allocate().ok_or(Error::NoMemory).and_then(|frame| {
                let result = initialize(frame.physical());
                object.frames[page] = Some(frame);
                result
            });
            if let Err(error) = result {
                Self::free(pool, &mut object);
                return Err(error);
            }
        }
        let slot = &mut self.slots[index];
        slot.object = Some(object);
        Ok(MemoryId {
            slot: index,
            generation: slot.generation,
        })
    }

    pub fn pages(&self, id: MemoryId) -> Result<usize, Error> {
        Ok(self.object(id)?.pages)
    }

    pub fn retain(&mut self, id: MemoryId) -> Result<(), Error> {
        let object = self.object_mut(id)?;
        if object.caps == 0 {
            return Err(Error::InvalidHandle);
        }
        object.caps = object.caps.checked_add(1).ok_or(Error::NoSpace)?;
        Ok(())
    }

    pub fn release<const F: usize>(
        &mut self,
        id: MemoryId,
        pool: &mut FramePool<F>,
    ) -> Result<(), Error> {
        let object = self.object_mut(id)?;
        object.caps = object.caps.checked_sub(1).ok_or(Error::InvalidHandle)?;
        self.collect(id, pool);
        Ok(())
    }

    pub fn pin(&mut self, id: MemoryId) -> Result<MappingLease, Error> {
        let object = self.object_mut(id)?;
        if object.caps == 0 {
            return Err(Error::InvalidHandle);
        }
        object.mappings = object.mappings.checked_add(1).ok_or(Error::NoSpace)?;
        Ok(MappingLease { id })
    }

    pub fn physical(&self, lease: &MappingLease, page: usize) -> Result<u64, Error> {
        self.object(lease.id)?
            .frames
            .get(page)
            .and_then(Option::as_ref)
            .map(Frame::physical)
            .ok_or(Error::InvalidArgument)
    }

    /// The caller has already removed every translation covered by this lease.
    pub fn unpin<const F: usize>(&mut self, lease: MappingLease, pool: &mut FramePool<F>) {
        let object = self.object_mut(lease.id).expect("retained memory mapping");
        object.mappings = object.mappings.checked_sub(1).expect("mapping count");
        self.collect(lease.id, pool);
    }

    pub fn live_count(&self) -> usize {
        self.slots.iter().filter(|s| s.object.is_some()).count()
    }

    fn object(&self, id: MemoryId) -> Result<&Object<P>, Error> {
        let slot = self.slots.get(id.slot).ok_or(Error::InvalidHandle)?;
        if id.generation == 0 || slot.generation != id.generation {
            return Err(Error::InvalidHandle);
        }
        slot.object.as_ref().ok_or(Error::InvalidHandle)
    }

    fn object_mut(&mut self, id: MemoryId) -> Result<&mut Object<P>, Error> {
        self.object(id)?;
        Ok(self.slots[id.slot]
            .object
            .as_mut()
            .expect("validated memory object"))
    }

    fn collect<const F: usize>(&mut self, id: MemoryId, pool: &mut FramePool<F>) {
        let slot = &mut self.slots[id.slot];
        let object = slot.object.as_mut().expect("live memory object");
        if object.caps != 0 || object.mappings != 0 {
            return;
        }
        Self::free(pool, object);
        slot.object = None;
        slot.generation = slot.generation.checked_add(1).unwrap_or(0);
    }

    fn free<const F: usize>(pool: &mut FramePool<F>, object: &mut Object<P>) {
        for frame in object.frames.iter_mut().filter_map(Option::take) {
            pool.release(frame).expect("memory object frame ownership");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::FrameRange;

    fn pool<const N: usize>() -> FramePool<N> {
        let mut pool = FramePool::new();
        pool.initialize(&[FrameRange::from_region(4096, 4096 * 32).unwrap()])
            .unwrap();
        pool
    }

    #[test]
    fn mappings_retain_frames_after_all_handles_close() {
        let mut pool = pool::<4>();
        let mut objects = MemoryObjects::<1, 4>::new();
        let id = objects.create(&mut pool, 2, |_| Ok(())).unwrap();
        let a = objects.pin(id).unwrap();
        objects.retain(id).unwrap();
        let b = objects.pin(id).unwrap();
        assert_eq!(objects.physical(&a, 0), objects.physical(&b, 0));
        assert_eq!(objects.physical(&a, 2), Err(Error::InvalidArgument));
        assert_eq!(
            objects.physical(&a, usize::MAX),
            Err(Error::InvalidArgument)
        );
        objects.release(id, &mut pool).unwrap();
        objects.release(id, &mut pool).unwrap();
        assert_eq!(objects.retain(id), Err(Error::InvalidHandle));
        assert!(matches!(objects.pin(id), Err(Error::InvalidHandle)));
        assert_eq!(objects.release(id, &mut pool), Err(Error::InvalidHandle));
        objects.unpin(a, &mut pool);
        assert_eq!(pool.live_count(), 2);
        assert_eq!(objects.live_count(), 1);
        objects.unpin(b, &mut pool);
        assert_eq!(pool.live_count(), 0);
        assert_eq!(objects.pages(id), Err(Error::InvalidHandle));
        let next = objects.create(&mut pool, 1, |_| Ok(())).unwrap();
        assert_ne!(id, next);
        assert_eq!(objects.retain(id), Err(Error::InvalidHandle));
        objects.release(next, &mut pool).unwrap();
    }

    #[test]
    fn partial_allocation_and_initialization_failure_preserve_registry() {
        let mut pool = pool::<2>();
        let mut objects = MemoryObjects::<1, 4>::new();
        assert_eq!(
            objects.create(&mut pool, 3, |_| Ok(())),
            Err(Error::NoMemory)
        );
        assert_eq!(pool.live_count(), 0);
        assert_eq!(objects.live_count(), 0);
        let mut calls = 0;
        assert_eq!(
            objects.create(&mut pool, 2, |_| {
                calls += 1;
                if calls == 2 {
                    Err(Error::InvalidArgument)
                } else {
                    Ok(())
                }
            }),
            Err(Error::InvalidArgument)
        );
        assert_eq!(pool.live_count(), 0);
        let id = objects.create(&mut pool, 2, |_| Ok(())).unwrap();
        assert_eq!(id.generation, 1);
        assert_eq!(
            objects.create(&mut pool, 1, |_| Ok(())),
            Err(Error::NoSpace)
        );
        objects.release(id, &mut pool).unwrap();
    }

    #[test]
    fn validates_sizes_and_retires_generations_without_wrap() {
        let mut pool = pool::<4>();
        let mut objects = MemoryObjects::<1, 4>::new();
        for pages in [0, 5, u64::MAX, 1 << 32] {
            assert_eq!(
                objects.create(&mut pool, pages, |_| unreachable!()),
                Err(Error::InvalidArgument)
            );
        }
        objects.slots[0].generation = u64::MAX;
        let id = objects.create(&mut pool, 1, |_| Ok(())).unwrap();
        objects.object_mut(id).unwrap().caps = u16::MAX;
        assert_eq!(objects.retain(id), Err(Error::NoSpace));
        objects.object_mut(id).unwrap().caps = 1;
        objects.object_mut(id).unwrap().mappings = u16::MAX;
        assert!(matches!(objects.pin(id), Err(Error::NoSpace)));
        objects.object_mut(id).unwrap().mappings = 0;
        objects.release(id, &mut pool).unwrap();
        assert_eq!(
            objects.create(&mut pool, 1, |_| Ok(())),
            Err(Error::NoSpace)
        );
        assert_eq!(
            MemoryObjects::<0, 4>::new().create(&mut pool, 1, |_| Ok(())),
            Err(Error::NoSpace)
        );
        assert_eq!(pool.live_count(), 0);
    }
}
