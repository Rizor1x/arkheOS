use super::{KernelSpace, MapError, NX, PAGE, UserSpace, WRITE, active_root, switch};
use arkhe_kernel_core::{elf::Executable, frame_pool::FramePool};

impl UserSpace {
    pub fn check_heap_lifecycle(
        &mut self,
        frames: &mut FramePool<{ super::POOL_FRAMES }>,
        kernel: &mut KernelSpace,
        image: &Executable<'_>,
    ) {
        use arkhe_abi::PRIVATE_HEAP_BASE;
        let baseline = frames.live_count();
        assert_eq!(self.resize_heap(frames, 1), Err(MapError::InvalidState));
        // SAFETY: The vacant test slot and pool are exclusively owned by this BSP.
        unsafe { self.load(frames, kernel, image) }.expect("heap probe image");
        assert_eq!(self.resize_heap(frames, 1), Err(MapError::InvalidState));
        let root = self.root();
        // SAFETY: The loaded root retains kernel text, stack, and all supervisor state.
        unsafe { switch(root) };
        self.resize_heap(frames, 1).expect("initial private page");
        self.copy_to_user(PRIVATE_HEAP_BASE, b"retained")
            .expect("heap write");
        let live = frames.live_count();
        let mut held = [const { None }; super::POOL_FRAMES];
        let mut count = 0;
        while let Some(frame) = frames.allocate() {
            held[count] = Some(frame);
            count += 1;
        }
        assert!(count > 2);
        for _ in 0..2 {
            count -= 1;
            frames
                .release(held[count].take().expect("held frame"))
                .expect("test ownership");
        }
        let exhausted = frames.live_count();
        assert_eq!(self.resize_heap(frames, 4), Err(MapError::NoMemory));
        assert_eq!(frames.live_count(), exhausted, "partial heap growth leaked");
        assert_eq!(self.heap.pages, 1);
        assert!(self.tables.walk(PRIVATE_HEAP_BASE + PAGE, false).is_err());
        let mut bytes = [0; 8];
        self.copy_from_user(PRIVATE_HEAP_BASE, &mut bytes)
            .expect("preserved heap");
        assert_eq!(&bytes, b"retained");
        for frame in held[..count].iter_mut() {
            frames
                .release(frame.take().expect("held frame"))
                .expect("test ownership");
        }
        assert_eq!(frames.live_count(), live);
        self.resize_heap(frames, 4)
            .expect("growth after exhaustion");
        self.resize_heap(frames, 0).expect("active heap shrink");
        assert_eq!(self.heap.pages, 0);
        assert!(self.tables.walk(PRIVATE_HEAP_BASE, false).is_err());
        self.resize_heap(frames, 2).expect("heap teardown fixture");
        // SAFETY: The retained kernel root shares this code/stack and flushes user translations.
        unsafe { switch(kernel.root()) };
        self.release(frames, kernel)
            .expect("release live private pages");
        assert_eq!(frames.live_count(), baseline);
        assert_eq!(self.heap.pages, 0);
        assert!(self.heap.frames.iter().all(Option::is_none));
    }

    /// # Safety
    /// The temporary pool exclusively owns usable RAM; no main-pool frame is live.
    /// The active bootloader root retains a writable direct map of those ranges.
    pub unsafe fn check_unsealed_rollback<const N: usize>(
        &mut self,
        frames: &mut FramePool<N>,
        direct_map: u64,
        image: &Executable<'_>,
    ) {
        assert_eq!(self.tables.count, 0);
        self.tables.direct_map = direct_map;
        // SAFETY: The caller grants exclusive RAM during this pre-kernel-root probe.
        let result = unsafe { self.tables.load_inner(frames, active_root(), image) };
        assert_eq!(result, Err(MapError::NoMemory));
        assert!(
            self.tables.count > 0,
            "load must allocate before exhaustion"
        );
        self.tables.release_frames(frames);
    }

    pub fn check_load_lifecycle<const N: usize>(
        &mut self,
        frames: &mut FramePool<N>,
        kernel: &mut KernelSpace,
        image: &Executable<'_>,
    ) {
        let baseline = frames.live_count();
        let baseline_users = kernel.user_roots();
        let mut stranger = KernelSpace::new();
        // SAFETY: A missing supervisor is rejected before RAM or hardware access.
        let result = unsafe { self.load(frames, &mut stranger, image) };
        assert_eq!(result, Err(MapError::WrongOwner));
        kernel.users = usize::MAX;
        // SAFETY: The valid owner is retained; the checked binding count refuses overflow.
        let result = unsafe { self.load(frames, kernel, image) };
        assert_eq!(kernel.users, usize::MAX);
        kernel.users = baseline_users;
        assert_eq!(result, Err(MapError::NoMemory));
        assert_eq!(frames.live_count(), baseline);
        assert_eq!(self.tables.count, 0);
        self.tables.direct_map = kernel.tables.direct_map;
        // SAFETY: This inactive test slot uses the main pool on the IRQ-masked BSP.
        unsafe { self.tables.load_inner(frames, kernel.tables.root, image) }
            .expect("unsealed test image");
        let mut executable = self.tables.owned[..self.tables.count]
            .iter()
            .enumerate()
            .filter(|(_, owned)| owned.as_ref().expect("owned frame").executable_at != 0);
        let first = executable.next().expect("first executable page").0;
        let second = executable.next().expect("second executable page").0;
        let owned = self.tables.owned[second].as_ref().expect("owned frame");
        let (table, index, entry) = self
            .tables
            .executable_leaf(owned)
            .expect("unsealed NX page");
        // An invalid later leaf forces failure after the first code alias was sealed.
        self.tables
            .write_entry(table, index, entry | WRITE)
            .expect("inject invalid inactive code leaf");
        assert_eq!(
            self.tables.finalize(&mut kernel.tables),
            Err(MapError::WritableCode)
        );
        let owned = self.tables.owned[first].as_ref().expect("owned frame");
        assert!(
            owned.sealed,
            "failure must follow a completed protection change"
        );
        let physical = owned.frame.physical();
        let alias = self.tables.pointer(physical).expect("code direct map") as u64;
        assert_eq!(
            kernel.supervisor_leaf(alias).expect("sealed alias") & WRITE,
            0
        );
        self.tables.release(frames, &mut kernel.tables);
        assert_eq!(
            frames.live_count(),
            baseline,
            "sealing rollback leaked frames"
        );
        assert_ne!(
            kernel.supervisor_leaf(alias).expect("restored alias") & WRITE,
            0
        );

        // Hold earlier allocations so the pool reaches the very frame just unsealed.
        loop {
            let reused = self.tables.allocate(frames).expect("reuse test frame");
            if reused == physical {
                let pointer = self.tables.pointer(reused).expect("reused direct map") as *mut u8;
                // SAFETY: The newly leased frame has no mappings usable by a process
                // and no Rust references. Writing primes the restored writable TLB entry.
                unsafe { pointer.write_volatile(0xa5) };
                self.tables
                    .zero(reused)
                    .expect("zero reused executable frame");
                for offset in 0..PAGE as usize {
                    // SAFETY: Every byte is inside this exclusively leased RAM page.
                    assert_eq!(unsafe { pointer.add(offset).read_volatile() }, 0);
                }
                break;
            }
        }
        self.tables.release_frames(frames);
        assert_eq!(frames.live_count(), baseline);

        for _ in 0..2 {
            // SAFETY: The inactive slot owns fresh leases under the retained kernel root.
            unsafe { self.load(frames, kernel, image) }.expect("reload sealed image");
            let mut code_pages = 0;
            for owned in self.tables.owned[..self.tables.count].iter().flatten() {
                if owned.executable_at == 0 {
                    continue;
                }
                let (_, _, entry) = self.tables.executable_leaf(owned).expect("sealed user RX");
                assert!(owned.sealed && entry & (WRITE | NX) == 0);
                let alias = self
                    .tables
                    .pointer(owned.frame.physical())
                    .expect("code alias") as u64;
                let leaf = kernel.supervisor_leaf(alias).expect("sealed code alias");
                assert_eq!(leaf & (WRITE | NX), NX);
                code_pages += 1;
            }
            assert!(code_pages >= 2);
            let live = frames.live_count();
            let root = self.root();
            assert_eq!(kernel.user_roots(), baseline_users + 1);
            // SAFETY: Reloading a non-vacant space is rejected before RAM or hardware access.
            let result = unsafe { self.load(frames, kernel, image) };
            assert_eq!(result, Err(MapError::InvalidState));
            assert_eq!(
                self.release(frames, &mut stranger),
                Err(MapError::WrongOwner)
            );
            assert_eq!(stranger.user_roots(), 0);
            // SAFETY: The loaded root retains this code/stack; IRQs stay masked.
            unsafe { switch(root) };
            assert_eq!(
                self.release(frames, kernel),
                Err(MapError::ActiveAddressSpace)
            );
            assert_eq!(active_root(), root);
            assert_eq!(frames.live_count(), live);
            assert_eq!(kernel.user_roots(), baseline_users + 1);
            let alias = self.executable_alias();
            assert_eq!(
                kernel
                    .supervisor_leaf(alias)
                    .expect("failed release preserved alias")
                    & WRITE,
                0
            );
            // SAFETY: The retained supervisor root owns the same kernel mappings.
            unsafe { switch(kernel.root()) };
            self.release(frames, kernel).expect("inactive test release");
            assert_eq!(kernel.user_roots(), baseline_users);
            assert_eq!(self.release(frames, kernel), Err(MapError::InvalidState));
            assert_eq!(
                self.copy_from_user(0x10000, &mut [0]),
                Err(MapError::InvalidState)
            );
            assert_eq!(
                self.copy_to_user(0x10000, b"x"),
                Err(MapError::InvalidState)
            );
            assert_eq!(self.validate_write(0x10000, 1), Err(MapError::InvalidState));
            assert_eq!(frames.live_count(), baseline, "reload leaked frames");
        }
    }

    pub fn executable_alias(&self) -> u64 {
        let owned = self.tables.owned[..self.tables.count]
            .iter()
            .flatten()
            .find(|owned| owned.sealed)
            .expect("sealed executable frame");
        self.tables
            .pointer(owned.frame.physical())
            .expect("executable alias") as u64
    }
}
