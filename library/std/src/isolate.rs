//! isolate crate
//!
#![stable(feature = "rust1", since = "1.0.0")]

use crate::sys::isolate;

cfg_if::cfg_if! {
    if #[cfg(bootstrap)]{
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

        #[stable(feature = "isolate_domain", since = "1.0.0")]
        #[allow(missing_docs)]
        pub fn context_enter<F,R, T>(mut f: F, t: T) -> R
        where F: FnMut(T) -> R
         {
            f(t)

        }
    }else{
        #[stable(feature = "isolate_domain", since = "1.0.0")]
        #[allow(missing_docs)]
        #[lang="domain_enter"]
        pub fn enter_domain() -> () {
            isolate::enter_domain()
        }

        #[stable(feature = "isolate_domain", since = "1.0.0")]
        #[allow(missing_docs)]
        #[lang="domain_exit"]
        pub fn exit_domain() -> () {
            isolate::exit_domain()
        }

        #[stable(feature = "isolate_domain", since = "1.0.0")]
        #[allow(missing_docs)]
        #[lang="context_enter"]
        pub fn context_enter<F,R, T>(mut f: F, t: T) -> R
        where F: FnMut(T) -> R
         {
            f(t)

        }
    }
}
