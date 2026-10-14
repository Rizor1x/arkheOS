use arkhe_kernel_core::memory::{FrameRange, RegionError};
use limine::BaseRevision;
use limine::memory_map::EntryType;
use limine::request::{HhdmRequest, MemoryMapRequest, RequestsEndMarker, RequestsStartMarker};

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
#[unsafe(link_section = ".requests_end_marker")]
static END: RequestsEndMarker = RequestsEndMarker::new();

pub struct BootMemory {
    ranges: [FrameRange; MAX_REGIONS],
    count: usize,
    pub direct_map_offset: u64,
}

#[derive(Debug)]
pub enum BootError {
    UnsupportedRevision,
    MissingMemoryMap,
    MissingDirectMap,
    TooManyRegions,
    InvalidRegion(RegionError),
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
        let empty = FrameRange::from_region(0, 0).map_err(BootError::InvalidRegion)?;
        let mut info = Self {
            ranges: [empty; MAX_REGIONS],
            count: 0,
            direct_map_offset,
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
