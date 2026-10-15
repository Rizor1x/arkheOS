use super::{Resource, State, Wait, block, status, wake};
use crate::arch::x86_64::exceptions::ExceptionFrame;
use arkhe_abi::{Error, Handle, MAX_MESSAGE_BYTES, Message};
use arkhe_kernel_core::capability::Rights;

pub(super) fn send(
    state: &mut State,
    slot: usize,
    handle: u64,
    pointer: u64,
    length: u64,
) -> Result<u64, Error> {
    let process = &state.processes[slot];
    let Resource::Sender(endpoint) = process
        .capabilities
        .resolve(Handle::from_raw(handle), Rights::SEND)?
    else {
        return Err(Error::PermissionDenied);
    };
    let length = usize::try_from(length).map_err(|_| Error::InvalidArgument)?;
    if length > MAX_MESSAGE_BYTES {
        return Err(Error::InvalidArgument);
    }
    let mut bytes = [0; MAX_MESSAGE_BYTES];
    if length != 0 {
        process
            .space
            .copy_from_user(pointer, &mut bytes[..length])
            .map_err(|_| Error::InvalidArgument)?;
    }
    state
        .endpoints
        .send(endpoint, Message::new(&bytes[..length])?)?;
    wake_receivers(state);
    Ok(length as u64)
}

pub(super) fn receive(
    state: &mut State,
    frame: &mut ExceptionFrame,
    slot: usize,
    handle: u64,
    pointer: u64,
    length: u64,
) {
    let (endpoint, capacity) = match receive_args(&state.processes[slot], handle, pointer, length) {
        Ok(args) => args,
        Err(error) => {
            frame.result(error as i64);
            return;
        }
    };
    let process = &state.processes[slot];
    let result = state.endpoints.receive_with(endpoint, capacity, |bytes| {
        process
            .space
            .copy_to_user(pointer, bytes)
            .map_err(|_| Error::InvalidArgument)
    });
    match result {
        Err(Error::WouldBlock) => {
            // The empty check and wait registration share one IRQ-masked entry,
            // so no sender can run between them and lose a wakeup on this BSP.
            block(
                state,
                frame,
                Wait::Receive {
                    endpoint,
                    pointer,
                    capacity,
                },
            );
        }
        result => frame.result(status(result.map(|length| length as u64))),
    }
}

fn receive_args(
    process: &super::Process,
    handle: u64,
    pointer: u64,
    length: u64,
) -> Result<(arkhe_kernel_core::endpoint::EndpointId, usize), Error> {
    let Resource::Receiver(endpoint) = process
        .capabilities
        .resolve(Handle::from_raw(handle), Rights::RECEIVE)?
    else {
        return Err(Error::PermissionDenied);
    };
    let capacity = usize::try_from(length).map_err(|_| Error::InvalidArgument)?;
    if capacity > MAX_MESSAGE_BYTES {
        return Err(Error::InvalidArgument);
    }
    process
        .space
        .validate_write(pointer, capacity)
        .map_err(|_| Error::InvalidArgument)?;
    Ok((endpoint, capacity))
}

pub(super) fn wake_receivers(state: &mut State) {
    for slot in 0..state.processes.len() {
        let Some(Wait::Receive {
            endpoint,
            pointer,
            capacity,
        }) = state.processes[slot].wait
        else {
            continue;
        };
        let process = &state.processes[slot];
        let result = state.endpoints.receive_with(endpoint, capacity, |bytes| {
            process
                .space
                .copy_to_user(pointer, bytes)
                .map_err(|_| Error::InvalidArgument)
        });
        if result != Err(Error::WouldBlock) {
            wake(state, slot, status(result.map(|n| n as u64)));
        }
    }
}

pub(super) fn release(state: &mut State, resource: Resource) -> bool {
    match resource {
        Resource::Sender(id) => {
            state.endpoints.close_sender(id).expect("owned sender");
            true
        }
        Resource::Receiver(id) => {
            state.endpoints.close_receiver(id).expect("owned receiver");
            true
        }
        _ => false,
    }
}
