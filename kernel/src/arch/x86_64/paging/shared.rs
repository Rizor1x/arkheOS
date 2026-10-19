use super::{ADDRESS, FramePool, MapError, NX, PAGE, PRESENT, USER, UserSpace, WRITE, active_root};
use arkhe_abi::{Error, SHARED_MEMORY_BASE, SHARED_MEMORY_MAX_PAGES};
use arkhe_kernel_core::memory_object::{MappingLease, MemoryId, MemoryObjects};

pub type SharedObjects = MemoryObjects<4, SHARED_MEMORY_MAX_PAGES>;

const _: () = assert!(SHARED_MEMORY_BASE.is_multiple_of(PAGE));
const _: () = assert!((SHARED_MEMORY_BASE & 0x1f_ffff) >= PAGE);
const _: () = assert!(
    (SHARED_MEMORY_BASE & 0x1f_ffff) + (SHARED_MEMORY_MAX_PAGES as u64 + 1) * PAGE <= 0x20_0000
);

pub(super) struct SharedMapping {
    lease: MappingLease,
    pages: usize,
    writable: bool,
}

impl UserSpace {
    pub fn create_shared<const N: usize>(
        &self,
        frames: &mut FramePool<N>,
        objects: &mut SharedObjects,
        pages: u64,
    ) -> Result<MemoryId, Error> {
        self.require_current().map_err(|_| Error::InvalidArgument)?;
        objects.create(frames, pages, |physical| {
            if physical == 0 || physical & !ADDRESS != 0 {
                return Err(Error::InvalidArgument);
            }
            self.tables
                .zero(physical)
                .map_err(|_| Error::InvalidArgument)
        })
    }

    /// One bounded NX window in the current process. The lease is retained until
    /// explicit unmap or inactive process teardown, independently of open handles.
    pub fn map_shared(
        &mut self,
        objects: &mut SharedObjects,
        id: MemoryId,
        writable: bool,
    ) -> Result<u64, Error> {
        self.require_current().map_err(|_| Error::InvalidArgument)?;
        if self.shared.is_some() {
            return Err(Error::NoSpace);
        }
        let pages = objects.pages(id)?;
        for page in 0..pages {
            if self
                .tables
                .read_entry(self.tables.shared_table, index(page))
                .map_err(|_| Error::InvalidArgument)?
                != 0
            {
                return Err(Error::InvalidArgument);
            }
        }
        // The complete window was preflighted; IRQ-off BSP serialization keeps it
        // unchanged. Object frames were zeroed and validated before publication.
        let lease = objects.pin(id)?;
        for page in 0..pages {
            let physical = objects
                .physical(&lease, page)
                .expect("retained shared frame");
            let flags = PRESENT | USER | NX | if writable { WRITE } else { 0 };
            self.tables
                .write_entry(self.tables.shared_table, index(page), physical | flags)
                .expect("preflight shared table");
            invalidate(page);
        }
        self.shared = Some(SharedMapping {
            lease,
            pages,
            writable,
        });
        Ok(SHARED_MEMORY_BASE)
    }

    /// Clear translations before returning the mapping lease. Process teardown
    /// may call this while inactive after switching CR3 with PCID/PGE disabled.
    pub fn unmap_shared<const N: usize>(
        &mut self,
        objects: &mut SharedObjects,
        frames: &mut FramePool<N>,
    ) -> Result<(), MapError> {
        self.require_loaded()?;
        let mapping = self.shared.as_ref().ok_or(MapError::InvalidState)?;
        for page in 0..mapping.pages {
            let physical = objects
                .physical(&mapping.lease, page)
                .expect("shared frame lease");
            let flags = PRESENT | USER | NX | if mapping.writable { WRITE } else { 0 };
            let entry = self
                .tables
                .read_entry(self.tables.shared_table, index(page))?;
            if entry & !(32 | 64) != physical | flags {
                return Err(MapError::InvalidState);
            }
        }
        let mapping = self.shared.take().expect("validated shared mapping");
        for page in 0..mapping.pages {
            self.tables
                .write_entry(self.tables.shared_table, index(page), 0)
                .expect("validated shared table");
            if active_root() == self.tables.root {
                invalidate(page);
            }
        }
        objects.unpin(mapping.lease, frames);
        Ok(())
    }

    pub fn has_shared_mapping(&self) -> bool {
        self.shared.is_some()
    }

    fn require_current(&self) -> Result<(), MapError> {
        self.require_loaded()?;
        if active_root() != self.tables.root {
            return Err(MapError::InvalidState);
        }
        Ok(())
    }
}

fn index(page: usize) -> usize {
    ((address(page) >> 12) & 511) as usize
}

fn address(page: usize) -> u64 {
    assert!(page < SHARED_MEMORY_MAX_PAGES);
    SHARED_MEMORY_BASE
        .checked_add((page as u64).checked_mul(PAGE).expect("shared page size"))
        .expect("shared page address")
}

fn invalidate(page: usize) {
    let address = address(page);
    // SAFETY: IRQ-masked ring0 BSP changes its own user leaf, PCID/PGE disabled.
    // The memory clobber orders PTE stores before invalidation and lease release.
    unsafe {
        core::arch::asm!("invlpg [{}]", in(reg) address, options(nostack, preserves_flags));
    }
}
