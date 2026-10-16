use super::{AddressSpace, MapError, NX, PAGE, WRITE};
use arkhe_kernel_core::{elf::Executable, frame_pool::FramePool};

impl AddressSpace {
    /// # Safety
    /// The temporary pool exclusively owns usable RAM; no main-pool frame is live.
    /// The active bootloader root retains a writable direct map of those ranges.
    pub unsafe fn check_unsealed_rollback<const N: usize>(
        &mut self,
        frames: &mut FramePool<N>,
        direct_map: u64,
        image: &Executable<'_>,
    ) {
        assert_eq!(self.count, 0);
        self.direct_map = direct_map;
        // SAFETY: The caller grants exclusive RAM during this pre-kernel-root probe.
        let result = unsafe { self.load_inner(frames, image) };
        assert_eq!(result, Err(MapError::NoMemory));
        assert!(self.count > 0, "load must allocate before exhaustion");
        self.release_frames(frames);
    }

    pub fn check_load_lifecycle<const N: usize>(
        &mut self,
        frames: &mut FramePool<N>,
        kernel: &mut AddressSpace,
        image: &Executable<'_>,
    ) {
        let baseline = frames.live_count();
        assert_eq!(self.count, 0);
        self.direct_map = kernel.direct_map;
        // SAFETY: This inactive test slot uses the main pool on the IRQ-masked BSP.
        unsafe { self.load_inner(frames, image) }.expect("unsealed test image");
        let mut executable = self.owned[..self.count]
            .iter()
            .enumerate()
            .filter(|(_, owned)| owned.as_ref().expect("owned frame").executable_at != 0);
        let first = executable.next().expect("first executable page").0;
        let second = executable.next().expect("second executable page").0;
        let owned = self.owned[second].as_ref().expect("owned frame");
        let (table, index, entry) = self.executable_leaf(owned).expect("unsealed NX page");
        // An invalid later leaf forces failure after the first code alias was sealed.
        self.write_entry(table, index, entry | WRITE)
            .expect("inject invalid inactive code leaf");
        assert_eq!(self.finalize(kernel), Err(MapError::WritableCode));
        let owned = self.owned[first].as_ref().expect("owned frame");
        assert!(
            owned.sealed,
            "failure must follow a completed protection change"
        );
        let physical = owned.frame.physical();
        let alias = self.pointer(physical).expect("code direct map") as u64;
        assert_eq!(
            kernel.supervisor_leaf(alias).expect("sealed alias") & WRITE,
            0
        );
        self.release(frames, kernel);
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
            let reused = self.allocate(frames).expect("reuse test frame");
            if reused == physical {
                let pointer = self.pointer(reused).expect("reused direct map") as *mut u8;
                // SAFETY: The newly leased frame has no mappings usable by a process
                // and no Rust references. Writing primes the restored writable TLB entry.
                unsafe { pointer.write_volatile(0xa5) };
                self.zero(reused).expect("zero reused executable frame");
                for offset in 0..PAGE as usize {
                    // SAFETY: Every byte is inside this exclusively leased RAM page.
                    assert_eq!(unsafe { pointer.add(offset).read_volatile() }, 0);
                }
                break;
            }
        }
        self.release_frames(frames);
        assert_eq!(frames.live_count(), baseline);

        for _ in 0..2 {
            // SAFETY: The inactive slot owns fresh leases under the retained kernel root.
            unsafe { self.load(frames, kernel, image) }.expect("reload sealed image");
            let mut code_pages = 0;
            for owned in self.owned[..self.count].iter().flatten() {
                if owned.executable_at == 0 {
                    continue;
                }
                let (_, _, entry) = self.executable_leaf(owned).expect("sealed user RX");
                assert!(owned.sealed && entry & (WRITE | NX) == 0);
                let alias = self.pointer(owned.frame.physical()).expect("code alias") as u64;
                let leaf = kernel.supervisor_leaf(alias).expect("sealed code alias");
                assert_eq!(leaf & (WRITE | NX), NX);
                code_pages += 1;
            }
            assert!(code_pages >= 2);
            self.release(frames, kernel);
            assert_eq!(frames.live_count(), baseline, "reload leaked frames");
        }
    }

    pub fn executable_alias(&self) -> u64 {
        let owned = self.owned[..self.count]
            .iter()
            .flatten()
            .find(|owned| owned.sealed)
            .expect("sealed executable frame");
        self.pointer(owned.frame.physical())
            .expect("executable alias") as u64
    }
}
