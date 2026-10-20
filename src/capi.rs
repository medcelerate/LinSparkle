//! C API (the `capi` feature), named after WinSparkle's: see
//! `include/linsparkle.h`. Strings are UTF-8 and only read during the call;
//! a null pointer is ignored.

use std::ffi::{CStr, c_char, c_int};

/// # Safety
/// `p` is null or a NUL-terminated string valid for the call.
unsafe fn string(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the caller passes a valid C string.
    Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

#[unsafe(no_mangle)]
pub extern "C" fn linsparkle_run_if_helper() {
    crate::run_if_helper();
}

/// # Safety
/// `url` is null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn linsparkle_set_appcast_url(url: *const c_char) {
    if let Some(url) = unsafe { string(url) } {
        crate::set_appcast_url(&url);
    }
}

/// # Safety
/// `pubkey` is null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn linsparkle_set_eddsa_public_key(pubkey: *const c_char) -> c_int {
    unsafe { string(pubkey) }.is_some_and(|k| crate::set_eddsa_public_key(&k)) as c_int
}

/// # Safety
/// Both are null or NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn linsparkle_set_app_details(app_name: *const c_char, app_version: *const c_char) {
    let name = unsafe { string(app_name) }.unwrap_or_default();
    let version = unsafe { string(app_version) }.unwrap_or_default();
    crate::set_app_details(&name, &version);
}

/// # Safety
/// `path` is null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn linsparkle_set_app_icon(path: *const c_char) {
    if let Some(path) = unsafe { string(path) } {
        crate::set_app_icon(path);
    }
}

/// # Safety
/// `path` is null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn linsparkle_set_install_path(path: *const c_char) {
    if let Some(path) = unsafe { string(path) } {
        crate::set_install_path(path);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn linsparkle_set_bundle_mode(enabled: c_int) {
    crate::set_bundle_mode(enabled != 0);
}

/// # Safety
/// `id` is null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn linsparkle_set_bundle_app_id(id: *const c_char) {
    if let Some(id) = unsafe { string(id) } {
        crate::set_bundle_app_id(&id);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn linsparkle_set_automatic_check_for_updates(state: c_int) {
    crate::set_automatic_check_for_updates(state != 0);
}

#[unsafe(no_mangle)]
pub extern "C" fn linsparkle_set_update_check_interval(interval: c_int) {
    crate::set_update_check_interval(interval.max(0) as u64);
}

#[unsafe(no_mangle)]
pub extern "C" fn linsparkle_set_shutdown_request_callback(callback: Option<extern "C" fn()>) {
    if let Some(callback) = callback {
        crate::set_shutdown_request_callback(move || callback());
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn linsparkle_init() {
    crate::init();
}

#[unsafe(no_mangle)]
pub extern "C" fn linsparkle_check_update_with_ui() {
    crate::check_update_with_ui();
}

#[unsafe(no_mangle)]
pub extern "C" fn linsparkle_check_update_without_ui() {
    crate::check_update_without_ui();
}

#[unsafe(no_mangle)]
pub extern "C" fn linsparkle_is_checking() -> c_int {
    crate::is_checking() as c_int
}

#[unsafe(no_mangle)]
pub extern "C" fn linsparkle_cleanup() {
    crate::cleanup();
}
