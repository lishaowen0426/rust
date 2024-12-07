//! context_switch crate
//!
#![stable(feature = "rust1", since = "1.0.0")]

use libc::c_void;

#[link(name = "context_switch")]
extern "C" {
    #[stable(feature = "isolate_domain", since = "1.0.0")]
    #[allow(missing_docs)]
    pub fn _context_switch(param: *mut c_void, fp: *mut c_void, next_stack: *mut c_void) -> ();

}

#[stable(feature = "isolate_domain", since = "1.0.0")]
#[cfg_attr(all(not(bootstrap)), lang = "context_switch")]
#[allow(missing_docs)]
#[inline(never)]
pub fn context_switch(param: *mut u8, fp: *mut u8, next_stack: *mut u8) -> () {
    unsafe {
        return _context_switch(param as *mut c_void, fp as *mut c_void, next_stack as *mut c_void);
    }
}
#[stable(feature = "isolate_domain", since = "1.0.0")]
#[cfg_attr(all(not(bootstrap)), lang = "transmute_to_ref")]
#[allow(missing_docs)]
#[inline(never)]
pub fn transmute_to_ref<'a, T>(p: *mut u8) -> &'a mut T {
    unsafe { &mut *(p as *mut T) }
}

#[stable(feature = "isolate_domain", since = "1.0.0")]
#[cfg_attr(all(not(bootstrap)), lang = "transmute_to_pointer")]
#[allow(missing_docs)]
#[inline(never)]
pub fn transmute_to_pointer<'a, T>(r: &mut T) -> *mut u8 {
    (r as *mut T).cast()
}

#[stable(feature = "isolate_domain", since = "1.0.0")]
#[allow(missing_docs)]
#[no_mangle]
#[inline(never)]
pub fn try_to_print() {
    println!("try_to_print");
}

#[stable(feature = "isolate_domain", since = "1.0.0")]
#[cfg_attr(all(not(bootstrap)), lang = "set_mimalloc_unsafe")]
#[allow(missing_docs)]
#[inline(never)]
#[linkage = "weak"]
#[no_mangle]
pub extern "C" fn set_rust_unsafe() {}

#[stable(feature = "isolate_domain", since = "1.0.0")]
#[cfg_attr(all(not(bootstrap)), lang = "clear_mimalloc_unsafe")]
#[allow(missing_docs)]
#[inline(never)]
#[linkage = "weak"]
#[no_mangle]
pub extern "C" fn clear_rust_unsafe() {}
