use core::{arch::asm, ptr};

fn syscall(number: usize, a: usize, b: usize, c: usize, d: usize) -> usize {
    let result;
    // Nyvela preserves general-purpose registers other than RAX across int 0x80.
    unsafe {
        asm!("int 0x80", inlateout("rax") number => result,
            in("rdi") a, in("rsi") b, in("rdx") c, in("r10") d);
    }
    result
}

pub fn exit(status: usize) -> ! {
    syscall(0, status, 0, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

pub fn log(bytes: &[u8]) {
    syscall(1, 1, bytes.as_ptr().addr(), bytes.len(), 0);
}

pub fn yield_now() {
    syscall(2, 0, 0, 0, 0);
}

pub fn send(tid: usize, bytes: &[u8]) {
    syscall(5, tid, bytes.as_ptr().addr(), bytes.len(), 0);
}

pub fn poll(bytes: &mut [u8], sender: &mut usize) -> usize {
    syscall(
        6,
        bytes.as_mut_ptr().addr(),
        bytes.len(),
        ptr::from_mut(sender).addr(),
        0,
    )
}

pub fn dma_alloc(physical: &mut usize) -> *mut u8 {
    ptr::with_exposed_provenance_mut(syscall(33, ptr::from_mut(physical).addr(), 0, 0, 0))
}

pub fn dma_free(page: *mut u8) {
    syscall(34, page.addr(), 0, 0, 0);
}

pub fn map_mmio(virtual_address: usize, physical: usize) -> bool {
    syscall(45, virtual_address, physical, 1, 0) == virtual_address
}

pub fn pci_read(bdf: usize, offset: usize) -> u32 {
    syscall(46, bdf >> 8, (bdf >> 3) & 31, bdf & 7, offset) as u32
}

pub fn tid() -> usize {
    syscall(47, 0, 0, 0, 0)
}

pub fn clock_ms() -> usize {
    syscall(48, 0, 0, 0, 0)
}

pub fn claim_nvme(bdf: usize) -> bool {
    syscall(49, bdf, 0, 0, 0) == 1
}

pub fn release_nvme() {
    syscall(49, 0, 1, 0, 0);
}
