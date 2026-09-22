//! The libvirt event callbacks. This is the only file in Lodger with
//! `unsafe` code, and the CI guard keeps it that way.
//!
//! `virt` wraps none of the event registration functions, so this module
//! calls them through `virt::sys`. The rules for every callback:
//!
//! - It copies the object's UUID and the event codes, calls `send` on the
//!   hub, and returns. It never blocks and never calls back into libvirt,
//!   except for the local `Get*UUIDString` read.
//! - It never wraps the lent object pointer in a `virt` type. The `Drop` of
//!   that type would free a handle that libvirt only lent.
//! - Its body runs inside `catch_unwind`, because a panic that crosses
//!   `extern "C"` aborts the process.
//!
//! Each registration gets its own boxed copy of the hub's sender as the
//! opaque pointer. Only libvirt's free callback, [`free_hub`], frees it.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use tokio::sync::broadcast::Sender;
use uuid::Uuid;
use virt::connect::Connect;
use virt::sys;

use super::{DomainChange, Event};

type Hub = Sender<Event>;

/// The event registrations on one connection. `Drop` removes them, and
/// libvirt then frees each opaque pointer through [`free_hub`].
pub(crate) struct Registration {
    /// A reference of its own, so the connection pointer stays valid until
    /// `Drop` finishes, whatever order the owner drops things in.
    conn: Connect,
    domain: Vec<c_int>,
    network: Vec<c_int>,
    pool: Vec<c_int>,
    close: bool,
}

/// Registers every callback on `conn`. On an error, the registrations made
/// so far are removed again.
///
/// If a register call fails, its opaque pointer leaks on purpose. libvirt
/// does not document whether it calls the free callback on a failure, and
/// a small leak is safe where a double free is not.
pub(crate) fn register(conn: &Connect, hub: &Hub) -> Result<Registration, virt::error::Error> {
    let mut reg = Registration {
        conn: conn.clone(),
        domain: Vec::new(),
        network: Vec::new(),
        pool: Vec::new(),
        close: false,
    };
    // SAFETY: `as_ptr` only reads the pointer. `reg.conn` keeps it valid.
    let ptr = unsafe { reg.conn.as_ptr() };
    let opaque = || Box::into_raw(Box::new(hub.clone())).cast::<c_void>();

    // SAFETY: each trampoline has the exact C signature that libvirt uses
    // for its event ID. libvirt's API takes them as the generic type and
    // casts back before the call (the `VIR_DOMAIN_EVENT_CALLBACK` macro).
    let domain_events: [(u32, sys::virConnectDomainEventGenericCallback); 5] = unsafe {
        [
            (
                sys::VIR_DOMAIN_EVENT_ID_LIFECYCLE,
                Some(std::mem::transmute::<DomainLifecycleFn, GenericFn>(
                    domain_lifecycle,
                )),
            ),
            (sys::VIR_DOMAIN_EVENT_ID_REBOOT, Some(domain_reboot)),
            (
                sys::VIR_DOMAIN_EVENT_ID_DEVICE_ADDED,
                Some(std::mem::transmute::<DomainDeviceFn, GenericFn>(
                    domain_device_added,
                )),
            ),
            (
                sys::VIR_DOMAIN_EVENT_ID_DEVICE_REMOVED,
                Some(std::mem::transmute::<DomainDeviceFn, GenericFn>(
                    domain_device_removed,
                )),
            ),
            (
                sys::VIR_DOMAIN_EVENT_ID_METADATA_CHANGE,
                Some(std::mem::transmute::<DomainMetadataFn, GenericFn>(
                    domain_metadata,
                )),
            ),
        ]
    };
    for (id, cb) in domain_events {
        // SAFETY: `ptr` is valid, a null domain means "every domain", and
        // `free_hub` matches the boxed `Hub` that `opaque` returns.
        let ret = unsafe {
            sys::virConnectDomainEventRegisterAny(
                ptr,
                ptr::null_mut(),
                id as c_int,
                cb,
                opaque(),
                Some(free_hub),
            )
        };
        reg.domain.push(check(ret)?);
    }

    // SAFETY: as above, for the network lifecycle signature.
    let ret = unsafe {
        sys::virConnectNetworkEventRegisterAny(
            ptr,
            ptr::null_mut(),
            sys::VIR_NETWORK_EVENT_ID_LIFECYCLE as c_int,
            Some(std::mem::transmute::<NetworkLifecycleFn, NetworkGenericFn>(
                network_lifecycle,
            )),
            opaque(),
            Some(free_hub),
        )
    };
    reg.network.push(check(ret)?);

    // SAFETY: as above, for the pool lifecycle signature.
    let ret = unsafe {
        sys::virConnectStoragePoolEventRegisterAny(
            ptr,
            ptr::null_mut(),
            sys::VIR_STORAGE_POOL_EVENT_ID_LIFECYCLE as c_int,
            Some(std::mem::transmute::<PoolLifecycleFn, PoolGenericFn>(
                pool_lifecycle,
            )),
            opaque(),
            Some(free_hub),
        )
    };
    reg.pool.push(check(ret)?);

    // The in-process test driver has no connection to lose. It returns 0
    // for a close callback but never stores it, so libvirt would never
    // free the box.
    if reg.conn.driver_type()? == "TEST" {
        return Ok(reg);
    }
    // SAFETY: `ptr` is valid and `on_close` has the `virConnectCloseFunc`
    // signature.
    let ret = unsafe {
        sys::virConnectRegisterCloseCallback(ptr, Some(on_close), opaque(), Some(free_hub))
    };
    check(ret)?;
    reg.close = true;
    Ok(reg)
}

fn check(ret: c_int) -> Result<c_int, virt::error::Error> {
    if ret < 0 {
        Err(virt::error::Error::last_error())
    } else {
        Ok(ret)
    }
}

impl Drop for Registration {
    /// Removes every callback. A failed removal is ignored: the connection
    /// is going away, and libvirt frees the rest when it closes.
    fn drop(&mut self) {
        // SAFETY: `self.conn` keeps the pointer valid, each ID came from
        // the matching register call, and `on_close` is the registered
        // close callback.
        unsafe {
            let ptr = self.conn.as_ptr();
            for id in self.domain.drain(..) {
                sys::virConnectDomainEventDeregisterAny(ptr, id);
            }
            for id in self.network.drain(..) {
                sys::virConnectNetworkEventDeregisterAny(ptr, id);
            }
            for id in self.pool.drain(..) {
                sys::virConnectStoragePoolEventDeregisterAny(ptr, id);
            }
            if self.close {
                sys::virConnectUnregisterCloseCallback(ptr, Some(on_close));
            }
        }
    }
}

type GenericFn = unsafe extern "C" fn(sys::virConnectPtr, sys::virDomainPtr, *mut c_void);
type DomainLifecycleFn =
    extern "C" fn(sys::virConnectPtr, sys::virDomainPtr, c_int, c_int, *mut c_void) -> c_int;
type DomainDeviceFn =
    extern "C" fn(sys::virConnectPtr, sys::virDomainPtr, *const c_char, *mut c_void);
type DomainMetadataFn =
    extern "C" fn(sys::virConnectPtr, sys::virDomainPtr, c_int, *const c_char, *mut c_void);
type NetworkGenericFn = unsafe extern "C" fn(sys::virConnectPtr, sys::virNetworkPtr, *mut c_void);
type NetworkLifecycleFn =
    extern "C" fn(sys::virConnectPtr, sys::virNetworkPtr, c_int, c_int, *mut c_void);
type PoolGenericFn = unsafe extern "C" fn(sys::virConnectPtr, sys::virStoragePoolPtr, *mut c_void);
type PoolLifecycleFn =
    extern "C" fn(sys::virConnectPtr, sys::virStoragePoolPtr, c_int, c_int, *mut c_void);

/// Builds the event inside `catch_unwind` and sends it to the hub behind
/// `opaque`. `send` never blocks. It fails only when nobody subscribes,
/// and then the event has no reader anyway.
fn deliver(opaque: *mut c_void, event: impl FnOnce() -> Option<Event>) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(event) = event() {
            // SAFETY: `opaque` is the boxed `Hub` from `register`. libvirt
            // calls `free_hub` only after the last callback returns.
            let hub = unsafe { &*opaque.cast::<Hub>() };
            let _ = hub.send(event);
        }
    }));
}

/// Parses the UUID string that a `Get*UUIDString` call wrote into `buf`.
fn parse_uuid(ret: c_int, buf: &[u8; sys::VIR_UUID_STRING_BUFLEN as usize]) -> Option<Uuid> {
    if ret < 0 {
        return None;
    }
    let text = CStr::from_bytes_until_nul(buf).ok()?.to_str().ok()?;
    Uuid::parse_str(text).ok()
}

fn domain_uuid(dom: sys::virDomainPtr) -> Option<Uuid> {
    let mut buf = [0u8; sys::VIR_UUID_STRING_BUFLEN as usize];
    // SAFETY: libvirt lends `dom` for the length of the callback, and
    // `buf` has the `VIR_UUID_STRING_BUFLEN` bytes that the call writes.
    let ret = unsafe { sys::virDomainGetUUIDString(dom, buf.as_mut_ptr().cast()) };
    parse_uuid(ret, &buf)
}

fn domain_event(opaque: *mut c_void, dom: sys::virDomainPtr, change: DomainChange) {
    deliver(opaque, || {
        Some(Event::Domain {
            id: domain_uuid(dom)?,
            change,
        })
    });
}

extern "C" fn domain_lifecycle(
    _conn: sys::virConnectPtr,
    dom: sys::virDomainPtr,
    event: c_int,
    detail: c_int,
    opaque: *mut c_void,
) -> c_int {
    domain_event(opaque, dom, DomainChange::Lifecycle { event, detail });
    0
}

extern "C" fn domain_reboot(
    _conn: sys::virConnectPtr,
    dom: sys::virDomainPtr,
    opaque: *mut c_void,
) {
    domain_event(opaque, dom, DomainChange::Reboot);
}

extern "C" fn domain_device_added(
    _conn: sys::virConnectPtr,
    dom: sys::virDomainPtr,
    _alias: *const c_char,
    opaque: *mut c_void,
) {
    domain_event(opaque, dom, DomainChange::DeviceAdded);
}

extern "C" fn domain_device_removed(
    _conn: sys::virConnectPtr,
    dom: sys::virDomainPtr,
    _alias: *const c_char,
    opaque: *mut c_void,
) {
    domain_event(opaque, dom, DomainChange::DeviceRemoved);
}

extern "C" fn domain_metadata(
    _conn: sys::virConnectPtr,
    dom: sys::virDomainPtr,
    _kind: c_int,
    _nsuri: *const c_char,
    opaque: *mut c_void,
) {
    domain_event(opaque, dom, DomainChange::Metadata);
}

extern "C" fn network_lifecycle(
    _conn: sys::virConnectPtr,
    net: sys::virNetworkPtr,
    event: c_int,
    _detail: c_int,
    opaque: *mut c_void,
) {
    deliver(opaque, || {
        let mut buf = [0u8; sys::VIR_UUID_STRING_BUFLEN as usize];
        // SAFETY: as in `domain_uuid`, for a lent network.
        let ret = unsafe { sys::virNetworkGetUUIDString(net, buf.as_mut_ptr().cast()) };
        Some(Event::Network {
            id: parse_uuid(ret, &buf)?,
            event,
        })
    });
}

extern "C" fn pool_lifecycle(
    _conn: sys::virConnectPtr,
    pool: sys::virStoragePoolPtr,
    event: c_int,
    _detail: c_int,
    opaque: *mut c_void,
) {
    deliver(opaque, || {
        let mut buf = [0u8; sys::VIR_UUID_STRING_BUFLEN as usize];
        // SAFETY: as in `domain_uuid`, for a lent storage pool.
        let ret = unsafe { sys::virStoragePoolGetUUIDString(pool, buf.as_mut_ptr().cast()) };
        Some(Event::Pool {
            id: parse_uuid(ret, &buf)?,
            event,
        })
    });
}

extern "C" fn on_close(_conn: sys::virConnectPtr, reason: c_int, opaque: *mut c_void) {
    deliver(opaque, || Some(Event::Closed { reason }));
}

/// libvirt's free callback for every opaque pointer from `register`.
extern "C" fn free_hub(opaque: *mut c_void) {
    let _ = catch_unwind(|| {
        // SAFETY: `opaque` came from `Box::into_raw` in `register`, and
        // libvirt calls the free callback once per registration.
        drop(unsafe { Box::from_raw(opaque.cast::<Hub>()) });
    });
}
