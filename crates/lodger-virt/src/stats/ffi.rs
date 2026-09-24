//! The call for the stats of every running domain. This is the second of the
//! 2 files in Lodger with `unsafe` code, next to `events/ffi.rs`, and the CI
//! guard keeps it that way.
//!
//! `virt` has `Connect::all_domain_stats`, but it returns records that safe
//! code cannot read, and it never frees them. This module makes the call
//! through `virt::sys`, copies each record into plain Rust values, and frees
//! the whole list with `virDomainStatsRecordListFree`. The rules:
//!
//! - The [`List`] guard owns the list from the moment the call returns, so
//!   the list is freed on every path, a panic included.
//! - Nothing keeps a pointer into the list after the copy. The domain
//!   pointer of a record is never wrapped in a `virt` type, whose `Drop`
//!   would free a reference that the list owns.
//! - Only the local `virDomainGetUUID` read calls back into libvirt.

use std::ffi::{CStr, c_uint};
use std::ptr;

use lodger_core::model::StatValue;
use uuid::Uuid;
use virt::connect::Connect;
use virt::sys;

/// One domain's stats, copied out of libvirt's record.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Record {
    pub uuid: Uuid,
    /// Each named value, such as `("cpu.time", UInt(..))`, in libvirt's order.
    pub params: Vec<(String, StatValue)>,
}

/// The list that `virConnectGetAllDomainStats` allocated. `Drop` frees it with
/// every record, and the domain reference that each record holds.
struct List(*mut sys::virDomainStatsRecordPtr);

impl Drop for List {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from virConnectGetAllDomainStats, and
            // this is its only owner. The free function accepts the
            // NULL-terminated list as it was returned.
            unsafe { sys::virDomainStatsRecordListFree(self.0) };
        }
    }
}

/// Calls `virConnectGetAllDomainStats` on `conn` with the `stats` groups and
/// the `flags`, and returns a copy of every record.
pub(crate) fn all_domain_stats(
    conn: &Connect,
    stats: c_uint,
    flags: c_uint,
) -> Result<Vec<Record>, virt::error::Error> {
    let mut raw: *mut sys::virDomainStatsRecordPtr = ptr::null_mut();
    // SAFETY: `as_ptr` only reads the pointer, and `conn` keeps it valid for
    // the call. libvirt writes a list that the caller owns into `raw`.
    let count =
        unsafe { sys::virConnectGetAllDomainStats(conn.as_ptr(), stats, &raw mut raw, flags) };
    let list = List(raw);
    if count < 0 {
        return Err(virt::error::Error::last_error());
    }
    let mut records = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
    for i in 0..usize::try_from(count).unwrap_or(0) {
        // SAFETY: the list has `count` entries before its NULL end, and
        // `list` keeps them alive until the end of this function.
        let entry = unsafe { *list.0.add(i) };
        if entry.is_null() {
            break;
        }
        // SAFETY: a non-NULL entry points to a record that the list owns.
        let record = unsafe { &*entry };
        let Some(uuid) = domain_uuid(record.dom) else {
            continue;
        };
        let params = match usize::try_from(record.nparams) {
            Ok(n) if n > 0 && !record.params.is_null() => {
                // SAFETY: `params` points to `nparams` values that the record
                // owns, and the record lives as long as `list`.
                unsafe { std::slice::from_raw_parts(record.params, n) }
                    .iter()
                    .filter_map(copy_param)
                    .collect()
            }
            _ => Vec::new(),
        };
        records.push(Record { uuid, params });
    }
    Ok(records)
}

/// The UUID of a record's domain. `None` for a NULL pointer or a failed read.
fn domain_uuid(dom: sys::virDomainPtr) -> Option<Uuid> {
    if dom.is_null() {
        return None;
    }
    let mut bytes = [0u8; 16];
    // SAFETY: `dom` is a valid domain that the list holds a reference to, and
    // `bytes` has room for VIR_UUID_BUFLEN (16) bytes.
    let rc = unsafe { sys::virDomainGetUUID(dom, bytes.as_mut_ptr()) };
    (rc == 0).then(|| Uuid::from_bytes(bytes))
}

/// Copies one typed value. An unknown type, a name without its NUL end, or
/// a NULL string gives `None`.
fn copy_param(p: &sys::virTypedParameter) -> Option<(String, StatValue)> {
    // The name is a fixed array of C chars. Copying it needs no raw read.
    let field: [u8; 80] = p.field.map(|c| c as u8);
    let name = CStr::from_bytes_until_nul(&field)
        .ok()?
        .to_str()
        .ok()?
        .to_owned();
    let Ok(kind) = u32::try_from(p.type_) else {
        return None;
    };
    // SAFETY: `type_` names the union member that libvirt wrote, and each
    // arm reads only that member. For a string, libvirt stores a
    // NUL-terminated copy that the record owns, and the record lives until
    // this copy is done.
    let value = unsafe {
        match kind {
            sys::VIR_TYPED_PARAM_INT => StatValue::Int(p.value.i.into()),
            sys::VIR_TYPED_PARAM_UINT => StatValue::UInt(p.value.ui.into()),
            sys::VIR_TYPED_PARAM_LLONG => StatValue::Int(p.value.l),
            sys::VIR_TYPED_PARAM_ULLONG => StatValue::UInt(p.value.ul),
            sys::VIR_TYPED_PARAM_DOUBLE => StatValue::Double(p.value.d),
            sys::VIR_TYPED_PARAM_BOOLEAN => StatValue::Bool(p.value.b != 0),
            sys::VIR_TYPED_PARAM_STRING if !p.value.s.is_null() => {
                StatValue::Text(CStr::from_ptr(p.value.s).to_string_lossy().into_owned())
            }
            _ => return None,
        }
    };
    Some((name, value))
}
