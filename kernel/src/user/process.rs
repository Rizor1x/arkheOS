use super::{Process, Resource, State, Wait, block, wake};
use crate::arch::x86_64::{exceptions::ExceptionFrame, paging, paging::STACK_TOP};
use arkhe_abi::{Error, Handle, SPAWN_GRANTS_BYTES, SpawnGrants};
use arkhe_kernel_core::{
    capability::{CapabilityTable, Rights},
    elf::Executable,
};

pub(super) fn launch_supervisor(state: &mut State) {
    let process = &mut state.processes[0];
    let image = state.image.as_ref().expect("supervisor ELF");
    // SAFETY: The frame pool owns RAM exclusively; this vacant slot is not runnable.
    unsafe {
        process
            .space
            .load(&mut state.frames, state.direct_map, image)
    }
    .expect("supervisor address space");
    let console = process
        .capabilities
        .insert(Resource::Console, Rights::SEND)
        .expect("supervisor console");
    let root = process
        .capabilities
        .insert(Resource::Directory, Rights::OPEN.union(Rights::SPAWN))
        .expect("supervisor namespace");
    let factory = process
        .capabilities
        .insert(Resource::ChannelFactory, Rights::CREATE)
        .expect("channel factory capability");
    process.context = ExceptionFrame::user(
        image.entry(),
        STACK_TOP - 8,
        console.raw(),
        0,
        root.raw(),
        factory.raw(),
        0,
    );
    process.ticks = 0;
    state.scheduler.insert(0).expect("vacant supervisor slot");
}

pub(super) fn spawn(
    state: &mut State,
    parent: usize,
    image: u64,
    argument: u64,
    console: u64,
) -> Result<u64, Error> {
    let image = image_bytes(state, parent, image)?;
    create(
        state,
        parent,
        image,
        argument,
        SpawnGrants {
            console: Handle::from_raw(console),
            ..SpawnGrants::NONE
        },
    )
}

pub(super) fn spawn_with_grants(
    state: &mut State,
    parent: usize,
    image: u64,
    argument: u64,
    pointer: u64,
) -> Result<u64, Error> {
    let image = image_bytes(state, parent, image)?;
    let mut bytes = [0; SPAWN_GRANTS_BYTES];
    state.processes[parent]
        .space
        .copy_from_user(pointer, &mut bytes)
        .map_err(|_| Error::InvalidArgument)?;
    let grants = SpawnGrants::decode(&bytes)?;
    create(state, parent, image, argument, grants)
}

fn image_bytes(state: &State, parent: usize, handle: u64) -> Result<&'static [u8], Error> {
    let Resource::File(file) = state.processes[parent]
        .capabilities
        .resolve(Handle::from_raw(handle), Rights::EXECUTE)?
    else {
        return Err(Error::PermissionDenied);
    };
    file.executable().ok_or(Error::PermissionDenied)
}

fn validate_grants(
    capabilities: &CapabilityTable<8, Resource>,
    grants: SpawnGrants,
) -> Result<(), Error> {
    if grants.sender.raw() != 0 && grants.sender == grants.receiver {
        return Err(Error::InvalidArgument);
    }
    if grants.console.raw() != 0
        && !matches!(
            capabilities.resolve(grants.console, Rights::SEND)?,
            Resource::Console
        )
    {
        return Err(Error::PermissionDenied);
    }
    if grants.sender.raw() != 0
        && !matches!(
            capabilities.resolve(grants.sender, Rights::SEND)?,
            Resource::Sender(_)
        )
    {
        return Err(Error::PermissionDenied);
    }
    if grants.receiver.raw() != 0
        && !matches!(
            capabilities.resolve(grants.receiver, Rights::RECEIVE)?,
            Resource::Receiver(_)
        )
    {
        return Err(Error::PermissionDenied);
    }
    Ok(())
}

fn create(
    state: &mut State,
    parent: usize,
    bytes: &'static [u8],
    argument: u64,
    grants: SpawnGrants,
) -> Result<u64, Error> {
    validate_grants(&state.processes[parent].capabilities, grants)?;
    let image = Executable::parse(bytes).map_err(|_| Error::BadExecutable)?;
    let slot = state.scheduler.vacant_slot().ok_or(Error::NoSpace)?;
    let completion = state.completions.create()?;
    let handle = match state.processes[parent]
        .capabilities
        .insert(Resource::Child(completion), Rights::WAIT)
    {
        Ok(handle) => handle,
        Err(error) => {
            cancel(state, completion);
            return Err(error);
        }
    };
    let child = &mut state.processes[slot];
    // SAFETY: The pool owns all allocated frames, the slot is vacant, and the BSP
    // keeps IRQs masked until the complete context and grants are published.
    let loaded = unsafe {
        child
            .space
            .load(&mut state.frames, state.direct_map, &image)
    };
    if let Err(error) = loaded {
        state.processes[parent]
            .capabilities
            .remove(handle)
            .expect("reserved watcher");
        cancel(state, completion);
        return Err(match error {
            paging::MapError::NoMemory => Error::NoMemory,
            _ => Error::BadExecutable,
        });
    }
    // All fallible reservations precede moves. IRQs remain masked, source handles
    // were validated together, and the empty child table has room for all three.
    let [console, sender, receiver] = publish_grants(state, parent, slot, grants);
    let child = &mut state.processes[slot];
    child.context = ExceptionFrame::user(
        image.entry(),
        STACK_TOP - 8,
        console,
        argument,
        0,
        sender,
        receiver,
    );
    child.completion = Some(completion);
    child.ticks = 0;
    child.wait = None;
    state.scheduler.insert(slot).expect("reserved process slot");
    state.spawned = state.spawned.checked_add(1).expect("spawned process count");
    Ok(handle.raw())
}

fn publish_grants(state: &mut State, parent: usize, slot: usize, grants: SpawnGrants) -> [u64; 3] {
    let [parent, child] = state
        .processes
        .get_disjoint_mut([parent, slot])
        .expect("running parent and vacant child differ");
    let console = if grants.console.raw() == 0 {
        0
    } else {
        child
            .capabilities
            .insert(Resource::Console, Rights::SEND)
            .expect("empty child capability table")
            .raw()
    };
    [
        console,
        move_side(parent, child, grants.sender, Rights::SEND),
        move_side(parent, child, grants.receiver, Rights::RECEIVE),
    ]
}

fn move_side(parent: &mut Process, child: &mut Process, handle: Handle, rights: Rights) -> u64 {
    if handle.raw() == 0 {
        return 0;
    }
    parent
        .capabilities
        .move_to(handle, rights, &mut child.capabilities)
        .expect("validated side and reserved child capacity")
        .raw()
}

fn cancel(state: &mut State, id: arkhe_kernel_core::completion::CompletionId) {
    state.completions.detach(id).expect("reserved completion");
    state
        .completions
        .complete(id, 0)
        .expect("unpublished completion");
}

pub(super) fn wait(state: &mut State, frame: &mut ExceptionFrame, slot: usize, handle: Handle) {
    let id = match state.processes[slot]
        .capabilities
        .resolve(handle, Rights::WAIT)
    {
        Ok(Resource::Child(id)) => id,
        Ok(_) => {
            frame.result(Error::PermissionDenied as i64);
            return;
        }
        Err(error) => {
            frame.result(error as i64);
            return;
        }
    };
    match state.completions.status(id) {
        Ok(_) => frame.result(reap(state, slot, handle, id)),
        Err(Error::WouldBlock) => {
            // IRQs stay masked between the running check and wait registration.
            block(
                state,
                frame,
                Wait::Child {
                    completion: id,
                    handle,
                },
            );
        }
        Err(error) => frame.result(error as i64),
    }
}

fn reap(
    state: &mut State,
    slot: usize,
    handle: Handle,
    id: arkhe_kernel_core::completion::CompletionId,
) -> i64 {
    let outcome = state.completions.reap(id).expect("completed owned child");
    state.processes[slot]
        .capabilities
        .remove(handle)
        .expect("owned watcher handle");
    outcome as i64
}

pub(super) fn wake_waiters(state: &mut State) {
    for slot in 0..state.processes.len() {
        let Some(Wait::Child { completion, handle }) = state.processes[slot].wait else {
            continue;
        };
        if state.completions.status(completion).is_ok() {
            let outcome = reap(state, slot, handle, completion);
            wake(state, slot, outcome);
        }
    }
}
