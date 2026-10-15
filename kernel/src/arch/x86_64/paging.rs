use arkhe_kernel_core::elf::Executable;
use arkhe_kernel_core::frame_pool::{Frame, FramePool};

pub const POOL_FRAMES: usize = 4096;
const MAX_OWNED_FRAMES: usize = 1100;

const PRESENT: u64 = 1;
const WRITE: u64 = 2;
const USER: u64 = 4;
const HUGE: u64 = 128;
const NX: u64 = 1 << 63;
const ADDRESS: u64 = 0x000f_ffff_ffff_f000;
const USER_LIMIT: u64 = 1 << 47;
const PAGE: u64 = 4096;
pub const STACK_TOP: u64 = 0x7000_0000;
const STACK_PAGES: u64 = 4;

#[derive(Debug, Eq, PartialEq)]
pub enum MapError {
    NoMemory,
    InvalidAddress,
    AlreadyMapped,
    UnexpectedHugePage,
    WritableCode,
}

pub struct AddressSpace {
    root: u64,
    direct_map: u64,
    owned: [Option<Frame>; MAX_OWNED_FRAMES],
    count: usize,
}

impl AddressSpace {
    pub const fn new() -> Self {
        Self {
            root: 0,
            direct_map: 0,
            owned: [const { None }; MAX_OWNED_FRAMES],
            count: 0,
        }
    }

    /// # Safety
    /// The allocator must exclusively own usable RAM mapped at `direct_map` by Limine.
    /// No other CPU may change the inherited kernel mappings while this space is alive.
    pub unsafe fn load<const N: usize>(
        &mut self,
        frames: &mut FramePool<N>,
        direct_map: u64,
        elf: &Executable<'_>,
    ) -> Result<(), MapError> {
        assert_eq!(self.count, 0, "address space already owns frames");
        self.direct_map = direct_map;
        // SAFETY: The caller provides exclusive usable RAM and unchanged kernel mappings.
        let result = unsafe { self.load_inner(frames, elf) };
        if result.is_err() {
            self.release(frames);
        }
        result
    }

    unsafe fn load_inner<const N: usize>(
        &mut self,
        frames: &mut FramePool<N>,
        elf: &Executable<'_>,
    ) -> Result<(), MapError> {
        let old_root: u64;
        let control: u64;
        // SAFETY: Ring 0 reads CR3/CR4; the active root is retained by Limine.
        unsafe {
            core::arch::asm!("mov {}, cr3", out(reg) old_root, options(nomem, nostack));
            core::arch::asm!("mov {}, cr4", out(reg) control, options(nomem, nostack));
        }
        if control & (1 << 12) != 0 || self.direct_map & (PAGE - 1) != 0 {
            return Err(MapError::InvalidAddress);
        }
        enable_protection();
        let root = self.allocate(frames)?;
        self.root = root;
        self.zero(root)?;
        for index in 256..512 {
            let value = self.read_entry(old_root & ADDRESS, index)? & !USER;
            self.write_entry(root, index, value)?;
        }
        for segment in elf.segments() {
            let start = segment.address & !(PAGE - 1);
            let end = (segment.address + segment.memory_size + PAGE - 1) & !(PAGE - 1);
            for virtual_page in (start..end).step_by(PAGE as usize) {
                self.map(frames, virtual_page, segment.writable, segment.executable)?;
            }
            self.copy_into(segment.address, segment.data)?;
        }
        for page in (STACK_TOP - STACK_PAGES * PAGE..STACK_TOP).step_by(PAGE as usize) {
            self.map(frames, page, true, false)?;
        }
        Ok(())
    }

    /// Switch CR3 away from this root before returning any of its frames.
    pub fn release<const N: usize>(&mut self, frames: &mut FramePool<N>) {
        assert_ne!(
            active_root(),
            self.root,
            "cannot release the active address space"
        );
        for frame in self.owned[..self.count].iter_mut() {
            frames
                .release(frame.take().expect("owned frame"))
                .expect("frame ownership");
        }
        self.count = 0;
        self.root = 0;
    }

    fn allocate<const N: usize>(&mut self, frames: &mut FramePool<N>) -> Result<u64, MapError> {
        if self.count == self.owned.len() {
            return Err(MapError::NoMemory);
        }
        let frame = frames.allocate().ok_or(MapError::NoMemory)?;
        let physical = frame.physical();
        if physical == 0 || physical & !ADDRESS != 0 {
            frames.release(frame).expect("rejected frame ownership");
            return Err(MapError::InvalidAddress);
        }
        self.owned[self.count] = Some(frame);
        self.count += 1;
        Ok(physical)
    }

    pub fn root(&self) -> u64 {
        self.root
    }

    pub fn copy_from_user(&self, address: u64, destination: &mut [u8]) -> Result<(), MapError> {
        let end = address
            .checked_add(destination.len() as u64)
            .ok_or(MapError::InvalidAddress)?;
        if address < 0x10000 || end > USER_LIMIT {
            return Err(MapError::InvalidAddress);
        }
        let mut done = 0;
        while done < destination.len() {
            let current = address + done as u64;
            let physical = self.translate(current)?;
            let length = destination
                .len()
                .saturating_sub(done)
                .min((PAGE - current % PAGE) as usize);
            let source = self.pointer(physical)? as *const u8;
            // SAFETY: The page walk validates user access at every level. IRQs are off,
            // the user cannot modify page tables, and the bounded destination is kernel-owned.
            unsafe {
                core::ptr::copy_nonoverlapping(source, destination[done..].as_mut_ptr(), length);
            }
            done += length;
        }
        Ok(())
    }

    /// Validate every destination page before writing any bytes. IRQs stay masked,
    /// and only this kernel owns the tables, so the second walk cannot race unmapping.
    pub fn copy_to_user(&self, address: u64, source: &[u8]) -> Result<(), MapError> {
        if source.is_empty() {
            return Ok(());
        }
        let end = address
            .checked_add(source.len() as u64)
            .ok_or(MapError::InvalidAddress)?;
        if address < 0x10000 || end > USER_LIMIT {
            return Err(MapError::InvalidAddress);
        }
        let mut done = 0;
        while done < source.len() {
            let current = address + done as u64;
            self.pointer(self.walk(current, true)?)?;
            done += (source.len() - done).min((PAGE - current % PAGE) as usize);
        }
        let mut done = 0;
        while done < source.len() {
            let current = address + done as u64;
            let destination = self.pointer(self.walk(current, true)?)? as *mut u8;
            let length = (source.len() - done).min((PAGE - current % PAGE) as usize);
            // SAFETY: Both walks require writable user access at every level. The
            // source is immutable kernel/module memory, disjoint from process-owned pages.
            // This BSP cannot change mappings while copying each bounded page chunk.
            unsafe {
                core::ptr::copy_nonoverlapping(source[done..].as_ptr(), destination, length);
            }
            done += length;
        }
        Ok(())
    }

    #[cfg(feature = "smoke")]
    pub fn check_copy_permissions(&mut self) {
        assert_ne!(
            active_root(),
            self.root,
            "permission probe requires an inactive root"
        );
        let address = STACK_TOP - PAGE;
        let physical = self.walk(address, true).expect("writable test stack");
        let byte = self.pointer(physical).expect("stack direct map") as *const u8;
        // SAFETY: This owned, zeroed stack page is inactive and has no Rust references.
        let before = unsafe { byte.read_volatile() };
        let mut table = self.root;
        for shift in [39, 30, 21, 12] {
            let index = ((address >> shift) & 511) as usize;
            let original = self.read_entry(table, index).expect("test page table");
            for permission in [PRESENT, USER, WRITE] {
                self.write_entry(table, index, original & !permission)
                    .expect("clear test permission");
                let result = self.copy_to_user(address, b"x");
                self.write_entry(table, index, original)
                    .expect("restore test permission");
                assert_eq!(
                    result,
                    Err(MapError::InvalidAddress),
                    "copy ignored page permission"
                );
                // SAFETY: Restored inactive mappings still own the same physical stack page.
                let after = unsafe { byte.read_volatile() };
                assert_eq!(after, before, "failed copy changed destination");
            }
            table = original & ADDRESS;
        }
    }

    fn map<const N: usize>(
        &mut self,
        frames: &mut FramePool<N>,
        address: u64,
        writable: bool,
        executable: bool,
    ) -> Result<(), MapError> {
        if !(0x10000..USER_LIMIT).contains(&address) || !address.is_multiple_of(PAGE) {
            return Err(MapError::InvalidAddress);
        }
        if writable && executable {
            return Err(MapError::WritableCode);
        }
        let mut table = self.root;
        for shift in [39, 30, 21] {
            let index = ((address >> shift) & 511) as usize;
            let mut entry = self.read_entry(table, index)?;
            if entry & PRESENT == 0 {
                let frame = self.allocate(frames)?;
                self.zero(frame)?;
                entry = frame | PRESENT | WRITE | USER;
                self.write_entry(table, index, entry)?;
            }
            if entry & HUGE != 0 {
                return Err(MapError::UnexpectedHugePage);
            }
            table = entry & ADDRESS;
        }
        let index = ((address >> 12) & 511) as usize;
        if self.read_entry(table, index)? & PRESENT != 0 {
            return Err(MapError::AlreadyMapped);
        }
        let frame = self.allocate(frames)?;
        self.zero(frame)?;
        let flags =
            PRESENT | USER | if writable { WRITE } else { 0 } | if executable { 0 } else { NX };
        self.write_entry(table, index, frame | flags)
    }

    fn translate(&self, address: u64) -> Result<u64, MapError> {
        self.walk(address, false)
    }

    fn walk(&self, address: u64, writable: bool) -> Result<u64, MapError> {
        if address >= USER_LIMIT {
            return Err(MapError::InvalidAddress);
        }
        let mut table = self.root;
        for shift in [39, 30, 21, 12] {
            let entry = self.read_entry(table, ((address >> shift) & 511) as usize)?;
            let required = PRESENT | USER | if writable { WRITE } else { 0 };
            if entry & required != required {
                return Err(MapError::InvalidAddress);
            }
            if shift != 12 && entry & HUGE != 0 {
                return Err(MapError::UnexpectedHugePage);
            }
            table = entry & ADDRESS;
        }
        Ok(table | (address & (PAGE - 1)))
    }

    fn copy_into(&self, address: u64, bytes: &[u8]) -> Result<(), MapError> {
        let mut done = 0;
        while done < bytes.len() {
            let current = address + done as u64;
            let physical = self.translate(current)?;
            let length = (bytes.len() - done).min((PAGE - current % PAGE) as usize);
            let destination = self.pointer(physical)? as *mut u8;
            // SAFETY: Newly allocated user pages are exclusively owned and not running.
            // The source is a validated ELF slice; each copy remains within one page.
            unsafe {
                core::ptr::copy_nonoverlapping(bytes[done..].as_ptr(), destination, length);
            }
            done += length;
        }
        Ok(())
    }

    fn pointer(&self, physical: u64) -> Result<usize, MapError> {
        let virtual_address = self
            .direct_map
            .checked_add(physical)
            .ok_or(MapError::InvalidAddress)?;
        if virtual_address >> 47 != 0x1ffff {
            return Err(MapError::InvalidAddress);
        }
        Ok(virtual_address as usize)
    }

    fn zero(&self, physical: u64) -> Result<(), MapError> {
        let pointer = self.pointer(physical)? as *mut u8;
        // SAFETY: The frame was obtained exclusively from usable RAM and mapped by HHDM.
        unsafe {
            core::ptr::write_bytes(pointer, 0, PAGE as usize);
        }
        Ok(())
    }

    fn read_entry(&self, table: u64, index: usize) -> Result<u64, MapError> {
        let address = self.pointer(table)? as *const u64;
        // SAFETY: Table frames are either retained Limine tables or owned zeroed pages;
        // indices are masked to 0..512, and HHDM preserves their page alignment.
        Ok(unsafe { address.add(index).read_volatile() })
    }

    fn write_entry(&self, table: u64, index: usize, value: u64) -> Result<(), MapError> {
        let address = self.pointer(table)? as *mut u64;
        // SAFETY: Only newly owned page-table frames are written, before user execution.
        unsafe {
            address.add(index).write_volatile(value);
        }
        Ok(())
    }
}

fn enable_protection() {
    let extended = core::arch::x86_64::__cpuid(0x8000_0000);
    assert!(extended.eax >= 0x8000_0001, "extended CPUID unavailable");
    let features = core::arch::x86_64::__cpuid(0x8000_0001);
    assert!(
        features.edx & (1 << 20) != 0,
        "NX is required for user execution"
    );
    let low: u32;
    let high: u32;
    // SAFETY: EFER exists in long mode. Enabling NX is supported by checked CPUID;
    // CR0.WP enforces supervisor write protection. EM/TS trap unsupported FP/SIMD.
    // Clearing PGE prevents stale global lower-half translations from surviving the upcoming CR3 switch.
    unsafe {
        core::arch::asm!("rdmsr", in("ecx") 0xc0000080u32, out("eax") low, out("edx") high);
        core::arch::asm!("wrmsr", in("ecx") 0xc0000080u32, in("eax") low | (1 << 11), in("edx") high);
        core::arch::asm!("mov rax, cr0", "or rax, 0x1000c", "mov cr0, rax",
            "mov rax, cr4", "and rax, -129", "mov cr4, rax", out("rax") _);
    }
}

pub fn active_root() -> u64 {
    let root: u64;
    // SAFETY: Only ring 0 calls this function; reading CR3 does not change mappings.
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) root, options(nomem, nostack));
    }
    root & ADDRESS
}

/// # Safety
/// The root must retain the running kernel, its stack, and all global kernel state.
pub unsafe fn switch(root: u64) {
    // SAFETY: The caller guarantees shared supervisor mappings; IRQs remain masked.
    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) root, options(nostack));
    }
}
