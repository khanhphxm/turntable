// Modified by Turntable: compact lazy arena chunks and quiescent/peak reuse; see ../TURNTABLE.md.

use std::{
    alloc::{self, handle_alloc_error},
    cell::Cell,
    num::NonZeroUsize,
    ops::{Deref, DerefMut},
    ptr::{self, NonNull},
    rc::Rc,
};

struct ArenaElement {
    value: *mut u8,
    drop: unsafe fn(*mut u8),
}

impl Drop for ArenaElement {
    #[inline(always)]
    fn drop(&mut self) {
        unsafe { (self.drop)(self.value) };
    }
}

struct Chunk {
    start: *mut u8,
    end: *mut u8,
    offset: *mut u8,
    alignment: usize,
}

impl Drop for Chunk {
    fn drop(&mut self) {
        unsafe {
            let chunk_size = self.end.addr() - self.start.addr();
            // SAFETY: This is the same layout used to allocate the chunk.
            let layout = alloc::Layout::from_size_align_unchecked(chunk_size, self.alignment);
            alloc::dealloc(self.start, layout);
        }
    }
}

impl Chunk {
    fn new(chunk_size: NonZeroUsize, alignment: usize) -> Self {
        // this only fails if chunk_size is unreasonably huge
        let layout = alloc::Layout::from_size_align(chunk_size.get(), alignment).unwrap();
        let start = unsafe { alloc::alloc(layout) };
        if start.is_null() {
            handle_alloc_error(layout);
        }
        let end = unsafe { start.add(chunk_size.get()) };
        Self {
            start,
            end,
            offset: start,
            alignment,
        }
    }

    fn allocate(&mut self, layout: alloc::Layout) -> Option<NonNull<u8>> {
        // Compute the allocation bounds in integer address space so that the
        // bounds check happens before any pointer offsetting. Offsetting a
        // pointer past the end of its allocation is undefined behavior even if
        // the result is never dereferenced (as happens on the chunk-spill
        // path), so `ptr::add` cannot be used until we know the result stays
        // in bounds. `checked_add` also handles the documented case where
        // `align_offset` returns `usize::MAX`.
        let base = self.offset.addr();
        let aligned_addr = base.checked_add(self.offset.align_offset(layout.align()))?;
        let next_addr = aligned_addr.checked_add(layout.size())?;

        if next_addr <= self.end.addr() {
            let aligned = self.offset.with_addr(aligned_addr);
            self.offset = self.offset.with_addr(next_addr);
            NonNull::new(aligned)
        } else {
            None
        }
    }

    fn reset(&mut self) {
        self.offset = self.start;
    }
}

pub struct Arena {
    chunks: Vec<Chunk>,
    elements: Vec<ArenaElement>,
    valid: Rc<Cell<bool>>,
    current_chunk_index: usize,
    small_clear_count: u8,
    chunk_size: NonZeroUsize,
    scope_depth: usize,
}

impl Drop for Arena {
    fn drop(&mut self) {
        self.force_clear();
    }
}

impl Arena {
    pub fn new(chunk_size: usize) -> Self {
        let chunk_size = NonZeroUsize::try_from(chunk_size).unwrap();
        Self {
            chunks: Vec::new(),
            elements: Vec::new(),
            valid: Rc::new(Cell::new(true)),
            current_chunk_index: 0,
            chunk_size,
            small_clear_count: 0,
            scope_depth: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.chunks
            .iter()
            .map(|chunk| chunk.end.addr() - chunk.start.addr())
            .sum()
    }

    /// Marks the start of a scope (e.g. a window draw) whose allocations must stay
    /// live until the scope ends, even if `clear` is called by a nested scope in
    /// the meantime.
    pub fn begin_scope(&mut self) {
        self.scope_depth += 1;
    }

    /// Ends the innermost scope started with `begin_scope`.
    ///
    /// Panics if no scope is active: an unbalanced `end_scope` would let `clear`
    /// run while an enclosing scope still references arena memory, which is
    /// exactly the use-after-free this bookkeeping exists to prevent, so failing
    /// loudly here is preferable.
    pub fn end_scope(&mut self) {
        self.scope_depth = self
            .scope_depth
            .checked_sub(1)
            .expect("Arena::end_scope called without a matching begin_scope");
    }

    /// Drops all allocations and resets the arena, unless a scope is still active.
    ///
    /// When a draw triggers a nested draw (e.g. re-entrant window procedure
    /// invocations on Windows, or opening a window from within a draw), the nested
    /// draw's clear must not free memory the outer draw still references, so it is
    /// deferred: the outer draw's own clear will drop both draws' allocations.
    pub fn clear(&mut self) {
        if self.scope_depth == 0 {
            self.force_clear();
        } else {
            log::debug!(
                "deferring arena clear; {} enclosing scope(s) still active",
                self.scope_depth
            );
        }
    }

    /// Releases backing storage only after every value and enclosing scope is gone.
    /// Unlike `clear`, an idle notification must never invalidate live allocations.
    pub(crate) fn release_idle_memory(&mut self) {
        if self.scope_depth != 0 || !self.elements.is_empty() {
            return;
        }
        self.chunks = Vec::new();
        self.elements = Vec::new();
        self.current_chunk_index = 0;
        self.small_clear_count = 0;
    }

    fn force_clear(&mut self) {
        self.valid.set(false);
        self.valid = Rc::new(Cell::new(true));
        let element_count = self.elements.len();
        let used_chunks = (self.current_chunk_index + 1).min(self.chunks.len());
        self.elements.clear();
        for chunk in self.chunks.iter_mut().take(used_chunks) {
            chunk.reset();
        }
        if self.chunks.len() > 4 && used_chunks * 4 < self.chunks.len() {
            self.small_clear_count += 1;
        } else {
            self.small_clear_count = 0;
        }
        if self.small_clear_count == 2 {
            // Retain a little headroom, but not a large page's unused tail.
            // All values have been dropped and active scopes defer this path.
            self.chunks.truncate(used_chunks.max(2));
            if self.elements.capacity() > element_count.max(128) * 4 {
                self.elements = Vec::with_capacity(element_count.max(128));
            }
            self.small_clear_count = 0;
        }
        self.current_chunk_index = 0;
    }

    #[inline(always)]
    pub fn alloc<T>(&mut self, f: impl FnOnce() -> T) -> ArenaBox<T> {
        #[inline(always)]
        unsafe fn inner_writer<T, F>(ptr: *mut T, f: F)
        where
            F: FnOnce() -> T,
        {
            unsafe { ptr::write(ptr, f()) };
        }

        unsafe fn drop<T>(ptr: *mut u8) {
            unsafe { std::ptr::drop_in_place(ptr.cast::<T>()) };
        }

        let layout = alloc::Layout::new::<T>();
        let ptr = if let Some(ptr) = self
            .chunks
            .get_mut(self.current_chunk_index)
            .and_then(|chunk| chunk.allocate(layout))
        {
            ptr.as_ptr()
        } else {
            self.allocate_next_chunk(layout)
        };

        unsafe { inner_writer(ptr.cast(), f) };
        self.elements.push(ArenaElement {
            value: ptr,
            drop: drop::<T>,
        });

        ArenaBox {
            ptr: ptr.cast(),
            valid: self.valid.clone(),
        }
    }

    #[inline(never)]
    fn allocate_next_chunk(&mut self, layout: alloc::Layout) -> *mut u8 {
        while self.current_chunk_index < self.chunks.len() {
            self.current_chunk_index += 1;
            if let Some(pointer) = self
                .chunks
                .get_mut(self.current_chunk_index)
                .and_then(|chunk| chunk.allocate(layout))
            {
                return pointer.as_ptr();
            }
        }

        // A large or over-aligned element gets its own chunk rather than making
        // every arena reserve enough space for the largest possible element.
        let chunk_size = NonZeroUsize::new(self.chunk_size.get().max(layout.size())).unwrap();
        let mut chunk = Chunk::new(chunk_size, layout.align());
        let pointer = chunk.allocate(layout).expect("new chunk fits its allocation");
        self.chunks.push(chunk);
        log::trace!(
            "increased element arena capacity to {}kb",
            self.capacity() / 1024,
        );
        pointer.as_ptr()
    }
}

pub struct ArenaBox<T: ?Sized> {
    ptr: *mut T,
    valid: Rc<Cell<bool>>,
}

impl<T: ?Sized> ArenaBox<T> {
    #[inline(always)]
    pub fn map<U: ?Sized>(mut self, f: impl FnOnce(&mut T) -> &mut U) -> ArenaBox<U> {
        ArenaBox {
            ptr: f(&mut self),
            valid: self.valid,
        }
    }

    #[track_caller]
    fn validate(&self) {
        assert!(
            self.valid.get(),
            "attempted to dereference an ArenaRef after its Arena was cleared"
        );
    }
}

impl<T: ?Sized> Deref for ArenaBox<T> {
    type Target = T;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        self.validate();
        unsafe { &*self.ptr }
    }
}

impl<T: ?Sized> DerefMut for ArenaBox<T> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.validate();
        unsafe { &mut *self.ptr }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use super::*;

    #[test]
    fn idle_releases_only_cleared_unscoped_storage() {
        let mut arena = Arena::new(64);
        let value = arena.alloc(|| 42u64);
        let capacity = arena.capacity();
        arena.release_idle_memory();
        assert_eq!(arena.capacity(), capacity);
        assert_eq!(*value, 42);

        arena.clear();
        assert!(!value.valid.get());
        arena.begin_scope();
        arena.release_idle_memory();
        assert_eq!(arena.capacity(), capacity);
        arena.end_scope();
        arena.release_idle_memory();
        assert_eq!(arena.capacity(), 0);
        assert_eq!(arena.chunks.capacity(), 0);
        assert_eq!(arena.elements.capacity(), 0);
        assert_eq!(arena.current_chunk_index, 0);

        assert_eq!(*arena.alloc(|| 7u64), 7);
        arena.clear();
        arena.release_idle_memory();
        assert_eq!(arena.capacity(), 0);
    }

    #[test]
    fn test_arena_reclaims_drop_records_after_dropping_values() {
        struct DropCounter(Rc<Cell<usize>>);
        impl Drop for DropCounter {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let drops = Rc::new(Cell::new(0));
        let mut arena = Arena::new(8);
        for _ in 0..1024 {
            arena.alloc(|| DropCounter(drops.clone()));
        }
        arena.clear();
        assert_eq!(drops.get(), 1024);
        for _ in 0..2 {
            arena.alloc(|| DropCounter(drops.clone()));
            arena.clear();
        }
        assert_eq!(drops.get(), 1026);
        assert_eq!(arena.capacity(), 16);
        assert_eq!(arena.elements.capacity(), 128);
    }

    #[test]
    fn test_arena_reclaims_peak_after_two_small_clears() {
        let mut arena = Arena::new(16);
        for _ in 0..6 {
            arena.alloc(|| [0u8; 16]);
        }
        arena.clear();
        assert_eq!(arena.capacity(), 96);

        arena.alloc(|| 1u8);
        arena.clear();
        assert_eq!(arena.capacity(), 96);
        arena.alloc(|| 2u8);
        arena.clear();
        assert_eq!(arena.capacity(), 32);
        assert_eq!(*arena.alloc(|| [3u8; 16]), [3; 16]);
    }

    #[test]
    fn test_arena_keeps_peak_for_alternating_use() {
        let mut arena = Arena::new(16);
        for count in [6, 1, 6, 1, 6] {
            for _ in 0..count {
                arena.alloc(|| [0u8; 16]);
            }
            arena.clear();
            assert_eq!(arena.capacity(), 96);
        }
    }

    #[test]
    fn test_arena() {
        let mut arena = Arena::new(1024);
        let a = arena.alloc(|| 1u64);
        let b = arena.alloc(|| 2u32);
        let c = arena.alloc(|| 3u16);
        let d = arena.alloc(|| 4u8);
        assert_eq!(*a, 1);
        assert_eq!(*b, 2);
        assert_eq!(*c, 3);
        assert_eq!(*d, 4);

        arena.clear();
        let a = arena.alloc(|| 5u64);
        let b = arena.alloc(|| 6u32);
        let c = arena.alloc(|| 7u16);
        let d = arena.alloc(|| 8u8);
        assert_eq!(*a, 5);
        assert_eq!(*b, 6);
        assert_eq!(*c, 7);
        assert_eq!(*d, 8);

        // Ensure drop gets called.
        let dropped = Rc::new(Cell::new(false));
        struct DropGuard(Rc<Cell<bool>>);
        impl Drop for DropGuard {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        arena.alloc(|| DropGuard(dropped.clone()));
        arena.clear();
        assert!(dropped.get());
    }

    #[test]
    fn test_arena_starts_empty() {
        let mut arena = Arena::new(1024);
        assert_eq!(arena.capacity(), 0);
        arena.clear();
        assert_eq!(arena.capacity(), 0);
        assert_eq!(*arena.alloc(|| 42u64), 42);
        assert_eq!(arena.capacity(), 1024);
    }

    #[test]
    fn test_arena_large_allocations() {
        let mut arena = Arena::new(8);
        assert_eq!(*arena.alloc(|| [7u8; 32]), [7; 32]);
        assert_eq!(*arena.alloc(|| 42u64), 42);
        arena.clear();
        assert_eq!(*arena.alloc(|| 1u64), 1);
        assert_eq!(*arena.alloc(|| [9u8; 64]), [9; 64]);
    }

    #[test]
    fn test_arena_overaligned_allocations() {
        #[repr(align(256))]
        struct Aligned(u8);

        let mut arena = Arena::new(8);
        assert_eq!(arena.alloc(|| Aligned(42)).0, 42);
        arena.clear();
        assert_eq!(arena.alloc(|| Aligned(7)).0, 7);
    }

    #[test]
    fn test_arena_grow() {
        let mut arena = Arena::new(8);
        arena.alloc(|| 1u64);
        arena.alloc(|| 2u64);

        assert_eq!(arena.capacity(), 16);

        arena.alloc(|| 3u32);
        arena.alloc(|| 4u32);

        assert_eq!(arena.capacity(), 24);
    }

    #[test]
    fn test_arena_alignment() {
        let mut arena = Arena::new(256);
        let x1 = arena.alloc(|| 1u8);
        let x2 = arena.alloc(|| 2u16);
        let x3 = arena.alloc(|| 3u32);
        let x4 = arena.alloc(|| 4u64);
        let x5 = arena.alloc(|| 5u64);

        assert_eq!(*x1, 1);
        assert_eq!(*x2, 2);
        assert_eq!(*x3, 3);
        assert_eq!(*x4, 4);
        assert_eq!(*x5, 5);

        assert_eq!(x1.ptr.align_offset(std::mem::align_of_val(&*x1)), 0);
        assert_eq!(x2.ptr.align_offset(std::mem::align_of_val(&*x2)), 0);
    }

    #[test]
    #[should_panic(expected = "attempted to dereference an ArenaRef after its Arena was cleared")]
    fn test_arena_use_after_clear() {
        let mut arena = Arena::new(16);
        let value = arena.alloc(|| 1u64);

        arena.clear();
        let _read_value = *value;
    }

    #[test]
    fn test_clear_deferred_while_scope_active() {
        struct DropCounter(Rc<Cell<usize>>);
        impl Drop for DropCounter {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let drops = Rc::new(Cell::new(0));
        let mut arena = Arena::new(1024);

        // Outer draw starts and allocates.
        arena.begin_scope();
        let outer = arena.alloc(|| 42u64);
        arena.alloc({
            let drops = drops.clone();
            || DropCounter(drops)
        });

        // Nested draw runs to completion and requests a clear.
        arena.begin_scope();
        let inner = arena.alloc(|| 7u64);
        arena.alloc({
            let drops = drops.clone();
            || DropCounter(drops)
        });
        arena.end_scope();
        arena.clear();

        // The clear must be deferred: the outer draw's allocations are still live.
        assert_eq!(*outer, 42);
        assert_eq!(*inner, 7);
        assert_eq!(drops.get(), 0);

        // Once the outer draw finishes, its clear drops both draws' allocations.
        arena.end_scope();
        arena.clear();
        assert_eq!(drops.get(), 2);
    }

    #[test]
    fn test_clear_without_scope_is_immediate() {
        let mut arena = Arena::new(1024);
        let value = arena.alloc(|| 1u64);
        assert_eq!(*value, 1);
        arena.clear();
        assert!(!value.valid.get());
    }

    #[test]
    #[should_panic(expected = "Arena::end_scope called without a matching begin_scope")]
    fn test_unbalanced_end_scope_panics() {
        let mut arena = Arena::new(1024);
        arena.begin_scope();
        arena.end_scope();
        arena.end_scope();
    }
}
