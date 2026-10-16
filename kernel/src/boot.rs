use arkhe_kernel_core::memory::{FrameRange, RegionError};
use limine::BaseRevision;
use limine::memory_map::EntryType;
use limine::request::{
    ExecutableAddressRequest, HhdmRequest, MemoryMapRequest, ModuleRequest, RequestsEndMarker,
    RequestsStartMarker,
};

pub const MAX_REGIONS: usize = 128;

#[used]
#[unsafe(link_section = ".requests_start_marker")]
static START: RequestsStartMarker = RequestsStartMarker::new();
#[used]
#[unsafe(link_section = ".requests")]
static REVISION: BaseRevision = BaseRevision::with_revision(3);
#[used]
#[unsafe(link_section = ".requests")]
static MEMORY: MemoryMapRequest = MemoryMapRequest::new();
#[used]
#[unsafe(link_section = ".requests")]
static HHDM: HhdmRequest = HhdmRequest::new();
#[used]
#[unsafe(link_section = ".requests")]
static MODULES: ModuleRequest = ModuleRequest::new();
#[used]
#[unsafe(link_section = ".requests")]
static EXECUTABLE: ExecutableAddressRequest = ExecutableAddressRequest::new();
#[used]
#[unsafe(link_section = ".requests_end_marker")]
static END: RequestsEndMarker = RequestsEndMarker::new();

pub struct BootMemory {
    ranges: [FrameRange; MAX_REGIONS],
    count: usize,
    pub direct_map_offset: u64,
    pub physical_base: u64,
    pub virtual_base: u64,
    pub module_address: u64,
    pub module_size: u64,
}

#[derive(Debug)]
pub enum BootError {
    UnsupportedRevision,
    MissingMemoryMap,
    MissingDirectMap,
    TooManyRegions,
    InvalidRegion(RegionError),
    MissingInitramfs,
    InvalidInitramfs,
    MissingExecutableAddress,
}

pub fn initramfs() -> Result<&'static [u8], BootError> {
    let modules = MODULES
        .get_response()
        .ok_or(BootError::MissingInitramfs)?
        .modules();
    if modules.len() != 1 {
        return Err(BootError::InvalidInitramfs);
    }
    let module = modules[0];
    let size = usize::try_from(module.size()).map_err(|_| BootError::InvalidInitramfs)?;
    if module.string().to_bytes() != b"initramfs"
        || size == 0
        || size > 16 * 1024 * 1024
        || module.addr().is_null()
        || (module.addr() as usize).checked_add(size).is_none()
    {
        return Err(BootError::InvalidInitramfs);
    }
    // SAFETY: Limine owns this immutable module mapping for the boot lifetime.
    // Its pages are not USABLE RAM; the allocator cannot reuse them. Length is bounded.
    Ok(unsafe { core::slice::from_raw_parts(module.addr(), size) })
}

impl BootMemory {
    pub fn read() -> Result<Self, BootError> {
        if !REVISION.is_supported() {
            return Err(BootError::UnsupportedRevision);
        }
        let memory = MEMORY.get_response().ok_or(BootError::MissingMemoryMap)?;
        let direct_map_offset = HHDM
            .get_response()
            .ok_or(BootError::MissingDirectMap)?
            .offset();
        let executable = EXECUTABLE
            .get_response()
            .ok_or(BootError::MissingExecutableAddress)?;
        let module = initramfs()?;
        let empty = FrameRange::from_region(0, 0).map_err(BootError::InvalidRegion)?;
        let mut info = Self {
            ranges: [empty; MAX_REGIONS],
            count: 0,
            direct_map_offset,
            physical_base: executable.physical_base(),
            virtual_base: executable.virtual_base(),
            module_address: module.as_ptr() as u64,
            module_size: module.len() as u64,
        };
        for entry in memory.entries() {
            if entry.entry_type != EntryType::USABLE {
                continue;
            }
            let range = FrameRange::from_region(entry.base, entry.length)
                .map_err(BootError::InvalidRegion)?;
            if range.count() == 0 {
                continue;
            }
            let slot = info
                .ranges
                .get_mut(info.count)
                .ok_or(BootError::TooManyRegions)?;
            *slot = range;
            info.count += 1;
        }
        Ok(info)
    }

    pub fn ranges(&self) -> &[FrameRange] {
        &self.ranges[..self.count]
    }
}
