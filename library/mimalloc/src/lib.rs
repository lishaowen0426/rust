#![no_std]
#![allow(unused_imports)]
#![allow(dead_code)]

use core::marker::{PhantomData, PhantomPinned, Send, Sync};
use libc::{c_void, size_t};

/*
mi_decl_nodiscard mi_decl_export mi_decl_restrict void *
mi_malloc(size_t size) mi_attr_noexcept mi_attr_malloc mi_attr_alloc_size(1);
mi_decl_nodiscard mi_decl_export mi_decl_restrict void *
mi_malloc_unsafe(size_t size) mi_attr_noexcept mi_attr_malloc
    mi_attr_alloc_size(1);
mi_decl_nodiscard mi_decl_export mi_decl_restrict void *
mi_calloc(size_t count, size_t size) mi_attr_noexcept mi_attr_malloc
    mi_attr_alloc_size2(1, 2);
mi_decl_nodiscard mi_decl_export void *
mi_realloc(void *p, size_t newsize) mi_attr_noexcept mi_attr_alloc_size(2);
mi_decl_export void *mi_expand(void *p, size_t newsize) mi_attr_noexcept
    mi_attr_alloc_size(2);

mi_decl_export void mi_free(void *p) mi_attr_noexcept;
*/

#[link(name = "mimalloc", kind = "static")]
extern "C" {
    pub fn mi_malloc(size: usize) -> *mut c_void;
    pub fn mi_zalloc(size: usize) -> *mut c_void;
    pub fn mi_malloc_unsafe(size: usize) -> *mut c_void;
    pub fn mi_calloc(count: usize, size: usize) -> *mut c_void;
    pub fn mi_realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
    pub fn mi_free(ptr: *mut c_void);

    pub fn mi_malloc_aligned(size: usize, alignment: usize) -> *mut c_void;
    pub fn mi_zalloc_aligned(size: usize, alignment: usize) -> *mut c_void;
    pub fn mi_realloc_aligned(ptr: *mut c_void, size: usize, alignment: usize) -> *mut c_void;

}
