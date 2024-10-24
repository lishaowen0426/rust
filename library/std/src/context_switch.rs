//! context_switch crate
//!
#![stable(feature = "rust1", since = "1.0.0")]

#[link(name = "context_switch")]
extern "C" {
    #[cfg(target_arch = "x86_64")]
    #[stable(feature = "isolate_domain", since = "1.0.0")]
    #[cfg_attr(all(not(bootstrap)), lang = "context_switch")]
    #[allow(missing_docs)]
    pub fn context_switch(param: *mut u8, fp: *mut u8, next_stack: *mut u8);

}

#[stable(feature = "isolate_domain", since = "1.0.0")]
#[cfg_attr(all(not(bootstrap)), lang = "transmute_to_ref")]
#[allow(missing_docs)]
pub fn transmute_to_ref<'a, T>(p: *mut u8) -> &'a mut T {
    unsafe { &mut *(p as *mut T) }
}

#[stable(feature = "isolate_domain", since = "1.0.0")]
#[cfg_attr(all(not(bootstrap)), lang = "transmute_to_pointer")]
#[allow(missing_docs)]
pub fn transmute_to_pointer<'a, T>(r: &mut T) -> *mut u8 {
    (r as *mut T).cast()
}
