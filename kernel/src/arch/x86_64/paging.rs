use arkhe_kernel_core::elf::Executable;
use arkhe_kernel_core::memory::FrameAllocator;

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

#[derive(Debug)]
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
}

impl AddressSpace {
    /// # Safety
    /// The allocator must exclusively own usable RAM mapped at `direct_map` by Limine.
    /// No other CPU may change the inherited kernel mappings while this space is alive.
    pub unsafe fn load(
        frames: &mut FrameAllocator<'_>,
        direct_map: u64,
        elf: &Executable<'_>,
    ) -> Result<Self, MapError> {
        let old_root: u64;
        let control: u64;
        // SAFETY: Ring 0 reads CR3/CR4; the active root is retained by Limine.
        unsafe {
            core::arch::asm!("mov {}, cr3", out(reg) old_root, options(nomem, nostack));
            core::arch::asm!("mov {}, cr4", out(reg) control, options(nomem, nostack));
        }
        if control & (1 << 12) != 0 || direct_map & (PAGE - 1) != 0 {
            return Err(MapError::InvalidAddress);
        }
        enable_protection();
        let root = next_frame(frames)?;
        let mut space = Self { root, direct_map };
        space.zero(root)?;
        for index in 256..512 {
            let value = space.read_entry(old_root & ADDRESS, index)? & !USER;
            space.write_entry(root, index, value)?;
        }
        for segment in elf.segments() {
            let start = segment.address & !(PAGE - 1);
            let end = (segment.address + segment.memory_size + PAGE - 1) & !(PAGE - 1);
            for virtual_page in (start..end).step_by(PAGE as usize) {
                space.map(frames, virtual_page, segment.writable, segment.executable)?;
            }
            space.copy_into(segment.address, segment.data)?;
        }
        for page in (STACK_TOP - STACK_PAGES * PAGE..STACK_TOP).step_by(PAGE as usize) {
            space.map(frames, page, true, false)?;
        }
        Ok(space)
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

    fn map(
        &mut self,
        frames: &mut FrameAllocator<'_>,
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
                let frame = next_frame(frames)?;
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
        let frame = next_frame(frames)?;
        self.zero(frame)?;
        let flags =
            PRESENT | USER | if writable { WRITE } else { 0 } | if executable { 0 } else { NX };
        self.write_entry(table, index, frame | flags)
    }

    fn translate(&self, address: u64) -> Result<u64, MapError> {
        if address >= USER_LIMIT {
            return Err(MapError::InvalidAddress);
        }
        let mut table = self.root;
        for shift in [39, 30, 21, 12] {
            let entry = self.read_entry(table, ((address >> shift) & 511) as usize)?;
            if entry & (PRESENT | USER) != PRESENT | USER {
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

fn next_frame(frames: &mut FrameAllocator<'_>) -> Result<u64, MapError> {
    loop {
        let frame = frames.allocate().ok_or(MapError::NoMemory)?;
        if frame != 0 && frame & !ADDRESS == 0 {
            return Ok(frame);
        }
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
    // CR0.WP enforces supervisor write protection. Clearing PGE prevents stale
    // global lower-half translations from surviving the upcoming CR3 switch.
    unsafe {
        core::arch::asm!("rdmsr", in("ecx") 0xc0000080u32, out("eax") low, out("edx") high);
        core::arch::asm!("wrmsr", in("ecx") 0xc0000080u32, in("eax") low | (1 << 11), in("edx") high);
        core::arch::asm!("mov rax, cr0", "or rax, 0x10000", "mov cr0, rax",
            "mov rax, cr4", "and rax, -129", "mov cr4, rax", out("rax") _);
    }
}
