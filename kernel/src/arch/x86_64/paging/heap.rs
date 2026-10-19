use super::{
    ADDRESS, Frame, FramePool, MapError, NX, PRESENT, USER, UserSpace, WRITE, active_root,
};
use arkhe_abi::{PRIVATE_HEAP_BASE, PRIVATE_HEAP_MAX_PAGES};
use arkhe_kernel_core::heap::{Resize, page_address};

pub(super) struct PrivateHeap {
    pub(super) frames: [Option<Frame>; PRIVATE_HEAP_MAX_PAGES],
    pub(super) pages: usize,
}

impl PrivateHeap {
    pub(super) const fn new() -> Self {
        Self {
            frames: [const { None }; PRIVATE_HEAP_MAX_PAGES],
            pages: 0,
        }
    }
}

impl UserSpace {
    /// Resize the caller's private RW/NX heap. Growth failure preserves mappings
    /// and contents; shrinking invalidates translations before returning leases.
    /// Only the current process can resize, on the IRQ-masked BSP without PCID/PGE.
    pub fn resize_heap<const N: usize>(
        &mut self,
        frames: &mut FramePool<N>,
        requested: u64,
    ) -> Result<u64, MapError> {
        self.require_loaded()?;
        if active_root() != self.tables.root {
            return Err(MapError::InvalidState);
        }
        self.resize_heap_inner(frames, requested)?;
        Ok(PRIVATE_HEAP_BASE)
    }

    pub(super) fn resize_heap_inner<const N: usize>(
        &mut self,
        frames: &mut FramePool<N>,
        requested: u64,
    ) -> Result<(), MapError> {
        let resize = Resize::new(self.heap.pages, requested).ok_or(MapError::InvalidSize)?;
        let table = self.tables.heap_table;
        if table == 0 {
            return Err(MapError::InvalidState);
        }
        // Bootstrap allocates this one leaf table before the process is published.
        // Every heap page and both guards fit in it; resizing never allocates tables.
        for page in resize.old.min(resize.new)..resize.old.max(resize.new) {
            let entry = self.tables.read_entry(table, index(page))?;
            let expected = if page < resize.old {
                self.heap.frames[page]
                    .as_ref()
                    .expect("heap frame")
                    .physical()
                    | PRESENT
                    | WRITE
                    | USER
                    | NX
            } else {
                0
            };
            // The CPU may set accessed/dirty; no other bits or physical aliases are allowed.
            if entry & !(32 | 64) != expected {
                return Err(MapError::InvalidState);
            }
        }
        if resize.new > resize.old {
            for page in resize.old..resize.new {
                if let Err(error) = self.prepare_heap_frame(frames, page) {
                    for pending in resize.old..page {
                        frames
                            .release(self.heap.frames[pending].take().expect("pending frame"))
                            .expect("heap rollback ownership");
                    }
                    return Err(error);
                }
            }
            for page in resize.old..resize.new {
                let physical = self.heap.frames[page]
                    .as_ref()
                    .expect("prepared heap frame")
                    .physical();
                self.tables
                    .write_entry(table, index(page), physical | PRESENT | WRITE | USER | NX)
                    .expect("preflight heap table");
                invalidate(page);
            }
        } else {
            for page in resize.new..resize.old {
                self.tables
                    .write_entry(table, index(page), 0)
                    .expect("preflight heap table");
                if active_root() == self.tables.root {
                    invalidate(page);
                }
                frames
                    .release(self.heap.frames[page].take().expect("released heap frame"))
                    .expect("heap ownership");
            }
        }
        self.heap.pages = resize.new;
        Ok(())
    }

    fn prepare_heap_frame<const N: usize>(
        &mut self,
        frames: &mut FramePool<N>,
        page: usize,
    ) -> Result<(), MapError> {
        let frame = frames.allocate().ok_or(MapError::NoMemory)?;
        let physical = frame.physical();
        let result = if physical == 0 || physical & !ADDRESS != 0 {
            Err(MapError::InvalidAddress)
        } else {
            self.tables.zero(physical)
        };
        if let Err(error) = result {
            frames.release(frame).expect("failed heap frame");
            return Err(error);
        }
        self.heap.frames[page] = Some(frame);
        Ok(())
    }
}

fn index(page: usize) -> usize {
    ((page_address(page).expect("bounded heap page") >> 12) & 511) as usize
}

fn invalidate(page: usize) {
    let address = page_address(page).expect("bounded heap invalidation");
    // SAFETY: The IRQ-masked BSP updates its current 4 KiB user leaf, with PCID/PGE
    // disabled. This memory clobber orders the PTE store before invalidation and reuse.
    unsafe {
        core::arch::asm!("invlpg [{}]", in(reg) address, options(nostack, preserves_flags));
    }
}
