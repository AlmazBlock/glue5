// Hooks for the runtime's `memory.rs` (see ../hooks.rs).
&[
    // The resource heap grows whenever the streaming engine asks how much
    // memory is left; on low-memory devices the host lets it grow only for
    // allocations (and only calls GetHeapSize, to know the heap's size).
    ("grow_heap", "rage::sysMemGrowBuddyAllocator::GrowHeap()"),
    ("heap_size", "rage::sysMemGrowBuddyAllocator::GetHeapSize() const"),
    ("memory_available", "rage::sysMemGrowBuddyAllocator::GetMemoryAvailable()"),
    ("largest_block", "rage::sysMemGrowBuddyAllocator::GetLargestAvailableBlock()"),
]
