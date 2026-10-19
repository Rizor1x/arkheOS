use super::{Resource, State};
use arkhe_abi::{Error, Handle};
use arkhe_kernel_core::capability::Rights;

pub(super) fn create(
    state: &mut State,
    slot: usize,
    pages: u64,
    b: u64,
    c: u64,
) -> Result<u64, Error> {
    if b != 0 || c != 0 {
        return Err(Error::InvalidArgument);
    }
    let id =
        state.processes[slot]
            .space
            .create_shared(&mut state.frames, &mut state.memory, pages)?;
    let rights = Rights::MAP.union(Rights::WRITE).union(Rights::GRANT);
    match state.processes[slot]
        .capabilities
        .insert(Resource::Memory(id), rights)
    {
        Ok(handle) => Ok(handle.raw()),
        Err(error) => {
            state
                .memory
                .release(id, &mut state.frames)
                .expect("unpublished memory object");
            Err(error)
        }
    }
}

pub(super) fn map(
    state: &mut State,
    slot: usize,
    handle: u64,
    writable: u64,
    reserved: u64,
) -> Result<u64, Error> {
    if writable > 1 || reserved != 0 {
        return Err(Error::InvalidArgument);
    }
    let rights = if writable == 0 {
        Rights::MAP
    } else {
        Rights::MAP.union(Rights::WRITE)
    };
    let Resource::Memory(id) = state.processes[slot]
        .capabilities
        .resolve(Handle::from_raw(handle), rights)?
    else {
        return Err(Error::PermissionDenied);
    };
    state.processes[slot]
        .space
        .map_shared(&mut state.memory, id, writable != 0)
}

pub(super) fn unmap(state: &mut State, slot: usize, a: u64, b: u64, c: u64) -> Result<u64, Error> {
    if a != 0 || b != 0 || c != 0 {
        return Err(Error::InvalidArgument);
    }
    state.processes[slot]
        .space
        .unmap_shared(&mut state.memory, &mut state.frames)
        .map(|()| 0)
        .map_err(|_| Error::InvalidArgument)
}
