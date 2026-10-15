use std::ffi::{c_char, CStr, CString};
use std::ptr;

use sweet_core::{Document, NodeId};

fn string(value: *const c_char) -> Result<String, String> {
    if value.is_null() {
        return Err("string argument is null".into());
    }
    unsafe { CStr::from_ptr(value) }
        .to_str()
        .map(str::to_owned)
        .map_err(|error| error.to_string())
}

fn node(id: u32) -> NodeId { NodeId(id) }

fn report(error: impl ToString, out_error: *mut *mut c_char) {
    if !out_error.is_null() {
        if let Ok(message) = CString::new(error.to_string()) {
            unsafe { *out_error = message.into_raw(); }
        }
    }
}

#[no_mangle]
pub extern "C" fn sweet_string_free(value: *mut c_char) {
    if !value.is_null() { unsafe { drop(CString::from_raw(value)); } }
}

#[no_mangle]
pub extern "C" fn sweet_document_new(error: *mut *mut c_char) -> *mut Document {
    match Document::new() {
        Ok(document) => Box::into_raw(Box::new(document)),
        Err(problem) => { report(problem, error); ptr::null_mut() }
    }
}

#[no_mangle]
pub extern "C" fn sweet_document_free(document: *mut Document) {
    if !document.is_null() { unsafe { drop(Box::from_raw(document)); } }
}

#[no_mangle]
pub extern "C" fn sweet_body(_document: *const Document) -> u32 { 0 }

#[no_mangle]
pub extern "C" fn sweet_create_element(document: *const Document, tag: *const c_char, error: *mut *mut c_char) -> u32 {
    let result = (|| { let tag = string(tag)?; unsafe { &*document }.create_element(&tag).map(|id| id.0).map_err(|e| e.to_string()) })();
    result.unwrap_or_else(|e| { report(e, error); u32::MAX })
}

#[no_mangle]
pub extern "C" fn sweet_create_text(document: *const Document, text: *const c_char, error: *mut *mut c_char) -> u32 {
    let result = (|| { let text = string(text)?; Ok::<u32, String>(unsafe { &*document }.create_text(&text).0) })();
    result.unwrap_or_else(|e| { report(e, error); u32::MAX })
}

#[no_mangle]
pub extern "C" fn sweet_append_child(document: *const Document, parent: u32, child: u32, error: *mut *mut c_char) -> bool {
    match unsafe { &*document }.append_child(node(parent), node(child)) { Ok(()) => true, Err(e) => { report(e, error); false } }
}

#[no_mangle]
pub extern "C" fn sweet_set_class(document: *const Document, id: u32, value: *const c_char, error: *mut *mut c_char) -> bool {
    let result = (|| { let value = string(value)?; unsafe { &*document }.set_class_name(node(id), &value).map_err(|e| e.to_string()) })();
    match result { Ok(()) => true, Err(e) => { report(e, error); false } }
}

#[no_mangle]
pub extern "C" fn sweet_set_text(document: *const Document, id: u32, value: *const c_char, error: *mut *mut c_char) -> bool {
    let result = (|| { let value = string(value)?; unsafe { &*document }.set_text_content(node(id), &value).map_err(|e| e.to_string()) })();
    match result { Ok(()) => true, Err(e) => { report(e, error); false } }
}

#[no_mangle]
pub extern "C" fn sweet_set_attribute(document: *const Document, id: u32, name: *const c_char, value: *const c_char, error: *mut *mut c_char) -> bool {
    let result = (|| { let name = string(name)?; let value = string(value)?; unsafe { &*document }.set_attribute(node(id), &name, &value).map_err(|e| e.to_string()) })();
    match result { Ok(()) => true, Err(e) => { report(e, error); false } }
}

#[no_mangle]
pub extern "C" fn sweet_render_png(document: *const Document, width: u32, height: u32, length: *mut usize, error: *mut *mut c_char) -> *mut u8 {
    match unsafe { &*document }.render_png(width, height) {
        Ok(bytes) => { unsafe { *length = bytes.len(); } let boxed = bytes.into_boxed_slice(); Box::into_raw(boxed) as *mut u8 }
        Err(problem) => { report(problem, error); ptr::null_mut() }
    }
}

#[no_mangle]
pub extern "C" fn sweet_bytes_free(value: *mut u8, length: usize) {
    if !value.is_null() { unsafe { drop(Box::from_raw(ptr::slice_from_raw_parts_mut(value, length))); } }
}

#[no_mangle]
pub extern "C" fn sweet_text_content(document: *const Document, id: u32, error: *mut *mut c_char) -> *mut c_char {
    match unsafe { &*document }.text_content(node(id)) {
        Ok(value) => CString::new(value).map(CString::into_raw).unwrap_or_else(|e| { report(e, error); ptr::null_mut() }),
        Err(problem) => { report(problem, error); ptr::null_mut() }
    }
}
