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
