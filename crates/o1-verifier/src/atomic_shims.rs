use core::ptr::{read_volatile, write_volatile};

#[no_mangle]
pub unsafe extern "C" fn __atomic_load_1(ptr: *const u8, _order: i32) -> u8 {
    read_volatile(ptr)
}

#[no_mangle]
pub unsafe extern "C" fn __atomic_load_8(ptr: *const u64, _order: i32) -> u64 {
    read_volatile(ptr)
}

#[no_mangle]
pub unsafe extern "C" fn __atomic_store_1(ptr: *mut u8, value: u8, _order: i32) {
    write_volatile(ptr, value);
}

#[no_mangle]
pub unsafe extern "C" fn __atomic_store_8(ptr: *mut u64, value: u64, _order: i32) {
    write_volatile(ptr, value);
}

#[no_mangle]
pub unsafe extern "C" fn __atomic_exchange_8(ptr: *mut u64, value: u64, _order: i32) -> u64 {
    let current = read_volatile(ptr);
    write_volatile(ptr, value);
    current
}

#[no_mangle]
pub unsafe extern "C" fn __atomic_compare_exchange_1(
    ptr: *mut u8,
    expected: *mut u8,
    desired: u8,
    _weak: bool,
    _success: i32,
    _failure: i32,
) -> bool {
    let current = read_volatile(ptr);
    if current == read_volatile(expected) {
        write_volatile(ptr, desired);
        true
    } else {
        write_volatile(expected, current);
        false
    }
}

#[no_mangle]
pub unsafe extern "C" fn __atomic_compare_exchange_8(
    ptr: *mut u64,
    expected: *mut u64,
    desired: u64,
    _weak: bool,
    _success: i32,
    _failure: i32,
) -> bool {
    let current = read_volatile(ptr);
    if current == read_volatile(expected) {
        write_volatile(ptr, desired);
        true
    } else {
        write_volatile(expected, current);
        false
    }
}

#[no_mangle]
pub unsafe extern "C" fn __atomic_fetch_add_8(ptr: *mut u64, value: u64, _order: i32) -> u64 {
    let current = read_volatile(ptr);
    write_volatile(ptr, current.wrapping_add(value));
    current
}

#[no_mangle]
pub unsafe extern "C" fn __atomic_fetch_sub_8(ptr: *mut u64, value: u64, _order: i32) -> u64 {
    let current = read_volatile(ptr);
    write_volatile(ptr, current.wrapping_sub(value));
    current
}

#[no_mangle]
pub unsafe extern "C" fn __atomic_fetch_or_8(ptr: *mut u64, value: u64, _order: i32) -> u64 {
    let current = read_volatile(ptr);
    write_volatile(ptr, current | value);
    current
}
