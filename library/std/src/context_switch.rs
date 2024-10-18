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
