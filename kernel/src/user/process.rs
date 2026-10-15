use super::{Resource, State, Wait, block, wake};
use crate::arch::x86_64::{exceptions::ExceptionFrame, paging, paging::STACK_TOP};
use arkhe_abi::{Error, Handle};
use arkhe_kernel_core::{capability::Rights, elf::Executable};

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
    process.context = ExceptionFrame::user(
        image.entry(),
        STACK_TOP - 8,
        console.raw(),
        0,
        root.raw(),
        0,
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
    let capabilities = &state.processes[parent].capabilities;
    let Resource::File(file) = capabilities.resolve(Handle::from_raw(image), Rights::EXECUTE)?
    else {
        return Err(Error::PermissionDenied);
    };
    if console != 0
        && !matches!(
            capabilities.resolve(Handle::from_raw(console), Rights::SEND)?,
            Resource::Console
        )
    {
        return Err(Error::PermissionDenied);
    }
    let image = Executable::parse(file.executable().ok_or(Error::PermissionDenied)?)
        .map_err(|_| Error::BadExecutable)?;
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
    let console = if console == 0 {
        0
    } else {
        child
            .capabilities
            .insert(Resource::Console, Rights::SEND)
            .expect("empty child capability table")
            .raw()
    };
    child.context = ExceptionFrame::user(image.entry(), STACK_TOP - 8, console, argument, 0, 0, 0);
    child.completion = Some(completion);
    child.ticks = 0;
    child.wait = None;
    state.scheduler.insert(slot).expect("reserved process slot");
    state.spawned = state.spawned.checked_add(1).expect("spawned process count");
    Ok(handle.raw())
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
