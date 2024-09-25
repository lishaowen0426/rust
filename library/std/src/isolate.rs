//! isolate crate
//!
#![stable(feature = "rust1", since = "1.0.0")]

use crate::sys::isolate;

#[stable(feature = "isolate_domain", since = "1.0.0")]
#[allow(missing_docs)]
pub fn enter_domain() -> () {
    isolate::enter_domain()
}

#[stable(feature = "isolate_domain", since = "1.0.0")]
#[allow(missing_docs)]
pub fn exit_domain() -> () {
    isolate::exit_domain()
}
