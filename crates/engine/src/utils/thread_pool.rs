//! `ThreadPool.h`: `parallel_for` helpers.
//!
//! The C++ engine runs loop bodies on a thread pool; every use is written so that iterations are
//! independent, which makes the result independent of scheduling. These helpers use scoped threads
//! with a shared work counter.

use crate::application::thread_count;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Run `f(i)` for every `i` in `begin..end`, possibly concurrently.
pub fn parallel_for<F>(begin: usize, end: usize, f: F)
where
    F: Fn(usize) + Sync,
{
    if end <= begin {
        return;
    }
    let n = end - begin;
    let threads = thread_count().min(n);
    if threads <= 1 {
        for i in begin..end {
            f(i);
        }
        return;
    }
    let next = AtomicUsize::new(begin);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= end {
                    break;
                }
                f(i);
            });
        }
    });
}

struct SyncPtr<T>(*mut T);
unsafe impl<T> Sync for SyncPtr<T> {}
unsafe impl<T> Send for SyncPtr<T> {}
impl<T> SyncPtr<T> {
    #[inline]
    fn get(&self) -> *mut T {
        self.0
    }
}

/// Run `f(i, &mut items[i])` for every element, possibly concurrently.
pub fn parallel_for_mut<T, F>(items: &mut [T], f: F)
where
    T: Send,
    F: Fn(usize, &mut T) + Sync,
{
    let n = items.len();
    let ptr = SyncPtr(items.as_mut_ptr());
    let ptr_ref = &ptr;
    parallel_for(0, n, |i| {
        // SAFETY: each index is visited exactly once, so the mutable borrows are disjoint.
        let item = unsafe { &mut *ptr_ref.get().add(i) };
        f(i, item);
    });
}

/// Parallel loop over indices where the body needs mutable access to distinct elements of a
/// slice through raw indexing (mirrors C++ lambdas capturing a container by reference and
/// writing only to `container[i]`).
pub struct UnsafeSlice<'a, T> {
    ptr: *mut T,
    len: usize,
    _marker: std::marker::PhantomData<&'a mut [T]>,
}
unsafe impl<T: Send> Sync for UnsafeSlice<'_, T> {}
unsafe impl<T: Send> Send for UnsafeSlice<'_, T> {}

impl<'a, T> UnsafeSlice<'a, T> {
    pub fn new(slice: &'a mut [T]) -> Self {
        UnsafeSlice { ptr: slice.as_mut_ptr(), len: slice.len(), _marker: std::marker::PhantomData }
    }

    /// # Safety
    /// The caller must guarantee that no two threads access the same index mutably at the same time.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get_mut(&self, i: usize) -> &mut T {
        assert!(i < self.len);
        &mut *self.ptr.add(i)
    }

    /// # Safety
    /// The caller must guarantee that the element is not being mutated concurrently.
    pub unsafe fn get(&self, i: usize) -> &T {
        assert!(i < self.len);
        &*self.ptr.add(i)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// A mutable reference that may be shared between the iterations of a parallel loop, for C++
/// code where each iteration only writes to its own part of the referenced data (e.g. one layer
/// of a mesh) while reading other, unmodified parts.
pub struct SharedMut<'a, T> {
    ptr: *mut T,
    _marker: std::marker::PhantomData<&'a mut T>,
}
unsafe impl<T> Sync for SharedMut<'_, T> {}
unsafe impl<T> Send for SharedMut<'_, T> {}

impl<'a, T> SharedMut<'a, T> {
    pub fn new(r: &'a mut T) -> Self {
        SharedMut { ptr: r as *mut T, _marker: std::marker::PhantomData }
    }

    /// # Safety
    /// Concurrent users must not write to the same data, nor read data that another user writes.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get(&self) -> &mut T {
        &mut *self.ptr
    }
}
