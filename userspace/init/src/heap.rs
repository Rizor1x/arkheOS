use crate::{
    arena::Arena,
    runtime::{call, exit},
};
use arkhe_abi::{Error, PRIVATE_HEAP_BASE, PRIVATE_HEAP_MAX_PAGES, Syscall, USER_PAGE_BYTES};
use core::sync::atomic::{AtomicBool, Ordering};

static CLAIMED: AtomicBool = AtomicBool::new(false);

/// Exclusive ownership of this single-threaded process's native heap mapping.
/// All arena buffers borrow this owner, preventing safe shrink or release while in use.
pub struct Heap {
    pages: usize,
}

impl Heap {
    pub fn new(pages: usize) -> Result<Self, Error> {
        if CLAIMED
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Err(Error::NoSpace);
        }
        // SAFETY: The successful claim excludes other Heap owners and arena borrows.
        let result = unsafe { crate::runtime::resize_heap(pages as u64, 0, 0) };
        if result != PRIVATE_HEAP_BASE as i64 {
            CLAIMED.store(false, Ordering::Release);
            return Err(if result == Error::NoMemory as i64 {
                Error::NoMemory
            } else {
                Error::InvalidArgument
            });
        }
        Ok(Self { pages })
    }

    pub fn arena(&mut self) -> Arena<'_> {
        // SAFETY: This owner alone controls the current process's successfully mapped
        // RW/NX pages. The mutable borrow prevents owner drop while arena buffers live.
        let bytes = unsafe {
            core::slice::from_raw_parts_mut(
                PRIVATE_HEAP_BASE as *mut u8,
                self.pages * USER_PAGE_BYTES,
            )
        };
        Arena::new(bytes)
    }
}

impl Drop for Heap {
    fn drop(&mut self) {
        // SAFETY: Exclusive owner drop cannot run while its arena buffers are borrowed.
        if unsafe { crate::runtime::resize_heap(0, 0, 0) } != PRIVATE_HEAP_BASE as i64 {
            exit(1);
        }
        CLAIMED.store(false, Ordering::Release);
    }
}

fn require(valid: bool) {
    if !valid {
        exit(1);
    }
}

/// # Safety
/// Run before creating Heap owners or retaining any references into the native heap.
pub unsafe fn check(console: u64, tag: u64) {
    let resize = |pages| {
        // SAFETY: The caller excludes heap owners/references; raw probes below use
        // only temporary pointers, and end before the scoped safe Heap example.
        unsafe { crate::runtime::resize_heap(pages, 0, 0) }
    };
    require(resize(0) == PRIVATE_HEAP_BASE as i64);
    for pages in [PRIVATE_HEAP_MAX_PAGES as u64 + 1, u64::MAX, 1 << 32] {
        require(resize(pages) == Error::InvalidArgument as i64);
    }
    // SAFETY: Reserved arguments must refuse allocation, with no heap borrows live.
    let bad = unsafe { crate::runtime::resize_heap(1, 1, 0) };
    require(bad == Error::InvalidArgument as i64);
    // SAFETY: The second reserved field must also refuse the operation before mapping.
    let bad = unsafe { crate::runtime::resize_heap(1, 0, 1) };
    require(bad == Error::InvalidArgument as i64);
    require(resize(PRIVATE_HEAP_MAX_PAGES as u64) == PRIVATE_HEAP_BASE as i64);
    for offset in 0..PRIVATE_HEAP_MAX_PAGES * USER_PAGE_BYTES {
        let pointer = (PRIVATE_HEAP_BASE as usize + offset) as *mut u8;
        // SAFETY: Every byte lies in this process's successfully mapped exclusive heap.
        unsafe {
            require(pointer.read_volatile() == 0);
            pointer.write_volatile((tag as u8) | 1);
        }
    }
    require(resize(PRIVATE_HEAP_MAX_PAGES as u64 + 1) == Error::InvalidArgument as i64);
    require(resize(1) == PRIVATE_HEAP_BASE as i64);
    require(
        call(
            Syscall::DebugWrite as u64,
            console,
            PRIVATE_HEAP_BASE + USER_PAGE_BYTES as u64,
            1,
        ) == if console == 0 {
            Error::InvalidHandle as i64
        } else {
            Error::InvalidArgument as i64
        },
    );
    require(resize(PRIVATE_HEAP_MAX_PAGES as u64) == PRIVATE_HEAP_BASE as i64);
    for offset in 0..PRIVATE_HEAP_MAX_PAGES * USER_PAGE_BYTES {
        let pointer = (PRIVATE_HEAP_BASE as usize + offset) as *const u8;
        let expected = if offset < USER_PAGE_BYTES {
            (tag as u8) | 1
        } else {
            0
        };
        // SAFETY: The regrown heap is mapped; new pages must be zero despite dirty reuse.
        unsafe {
            require(pointer.read_volatile() == expected);
        }
    }
    require(call(Syscall::Yield as u64, 0, 0, 0) == 0);
    // SAFETY: Yield retains this process's heap; another process must not alter it.
    unsafe {
        require((PRIVATE_HEAP_BASE as *const u8).read_volatile() == (tag as u8 | 1));
    }
    require(resize(0) == PRIVATE_HEAP_BASE as i64);
    {
        let mut heap = Heap::new(2).unwrap_or_else(|_| exit(1));
        require(matches!(Heap::new(1), Err(Error::NoSpace)));
        let mut arena = heap.arena();
        let a = arena
            .allocate(core::alloc::Layout::from_size_align(37, 64).unwrap())
            .unwrap_or_else(|| exit(1));
        let b = arena
            .allocate(core::alloc::Layout::from_size_align(4097, 256).unwrap())
            .unwrap_or_else(|| exit(1));
        a.fill(0x31);
        b.fill(0x72);
        require(a == [0x31; 37] && b.iter().all(|byte| *byte == 0x72));
        require(
            arena
                .allocate(core::alloc::Layout::from_size_align(8192, 1).unwrap())
                .is_none(),
        );
    }
    let _heap = Heap::new(1).unwrap_or_else(|_| exit(1));
}

/// # Safety
/// No Heap owner or heap references may exist. Leave pages for exit/fault cleanup.
pub unsafe fn leave_allocated() {
    // SAFETY: The caller grants raw control without outstanding heap borrows.
    require(unsafe { crate::runtime::resize_heap(3, 0, 0) } == PRIVATE_HEAP_BASE as i64);
}
