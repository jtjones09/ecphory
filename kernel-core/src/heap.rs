//! Kernel heap.
//!
//! For Phase 1 we carve the heap out of a fixed-size static region rather
//! than parsing the memory map and mapping pages. This keeps the boot path
//! linear and lets `alloc` work with no MMU surgery. Step 2 will replace
//! this with a real region carved from `BootInfo::memory_regions`.

use core::cell::UnsafeCell;
use linked_list_allocator::LockedHeap;

const HEAP_SIZE: usize = 32 * 1024 * 1024; // 32 MiB — enough for fabric + a 1920x1080 RGBA framebuffer copy on aarch64

#[repr(C, align(4096))]
struct HeapStorage(UnsafeCell<[u8; HEAP_SIZE]>);

unsafe impl Sync for HeapStorage {}

static HEAP_STORAGE: HeapStorage = HeapStorage(UnsafeCell::new([0; HEAP_SIZE]));

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init() {
    unsafe {
        let ptr = HEAP_STORAGE.0.get() as *mut u8;
        ALLOCATOR.lock().init(ptr, HEAP_SIZE);
    }
}

pub const fn size() -> usize {
    HEAP_SIZE
}
