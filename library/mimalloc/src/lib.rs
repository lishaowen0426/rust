#![no_std]
#![allow(unused_imports)]
#![allow(dead_code)]

use core::marker::{PhantomData, PhantomPinned};
use libc::{c_void, size_t};

#[repr(C)]
pub struct MiHeap {
    _data: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}
#[link(name = "mimalloc", kind = "static")]
extern "C" {
    fn mi_calloc(count: usize, size: usize) -> *mut u8;
    fn mi_heap_new() -> *mut MiHeap;
}
