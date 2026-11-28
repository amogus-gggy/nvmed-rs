use crate::{lumen, protocol::PAGE};
use core::{
    ptr::{self, read_volatile, write_volatile},
    sync::atomic::{fence, Ordering},
};

const MMIO: usize = 0x4000_0000;
const CAP: usize = 0;
const VS: usize = 8;
const INTMS: usize = 0x0c;
const CC: usize = 0x14;
const CSTS: usize = 0x1c;
const AQA: usize = 0x24;
const ASQ: usize = 0x28;
const ACQ: usize = 0x30;
const DOORBELLS: usize = 0x1000;
const COMMAND_TIMEOUT: usize = 30_000;

#[derive(Clone, Copy)]
pub enum Error {
    Unsupported,
    Allocation,
    Mapping,
    Timeout,
    Fatal,
    Completion,
    Command(u16),
}

impl Error {
    pub fn message(self) -> &'static [u8] {
        match self {
            Self::Unsupported => b"NVMED: unsupported controller or namespace\n",
            Self::Allocation => b"NVMED: DMA allocation failed\n",
            Self::Mapping => b"NVMED: BAR mapping failed\n",
            Self::Timeout => b"NVMED: controller timed out\n",
            Self::Fatal => b"NVMED: controller reported a fatal error\n",
            Self::Completion => b"NVMED: invalid completion\n",
            Self::Command(_) => b"NVMED: NVMe command failed\n",
        }
    }
}

pub fn discover() -> Option<(usize, usize)> {
    for bdf in 0..=0xffff {
        if lumen::pci_read(bdf, 0) & 0xffff == 0xffff || lumen::pci_read(bdf, 8) >> 8 != 0x010802
        {
            continue;
        }
        let low = lumen::pci_read(bdf, 0x10);
        let high = match low & 7 {
            0 => 0,
            4 => lumen::pci_read(bdf, 0x14) as usize,
            _ => continue,
        };
        let bar = (high << 32) | (low as usize & !0xf);
        if bar != 0 && bar % PAGE == 0 {
            return Some((bdf, bar));
        }
    }
    None
}

fn reg_read(offset: usize) -> u32 {
    unsafe { read_volatile(ptr::without_provenance(MMIO + offset)) }
}

fn reg_write(offset: usize, value: u32) {
    unsafe { write_volatile(ptr::without_provenance_mut(MMIO + offset), value) }
}

fn reg_write_address(offset: usize, value: usize) {
    reg_write(offset, value as u32);
    reg_write(offset + 4, (value >> 32) as u32);
}

struct Dma {
    physical: usize,
    virtual_address: *mut u8,
}

impl Dma {
    fn new() -> Result<Self, Error> {
        let mut physical = 0;
        let virtual_address = lumen::dma_alloc(&mut physical);
        if virtual_address.is_null() {
            return Err(Error::Allocation);
        }
        Ok(Self {
            physical,
            virtual_address,
        })
    }

    fn bytes(&self, offset: usize, output: &mut [u8]) {
        // The controller has completed its DMA before the CPU reads the buffer.
        for (index, byte) in output.iter_mut().enumerate() {
            *byte = unsafe { read_volatile(self.virtual_address.add(offset + index)) };
        }
    }

    fn write(&mut self, input: &[u8]) {
        for (index, &byte) in input.iter().enumerate() {
            unsafe { write_volatile(self.virtual_address.add(index), byte) };
        }
    }

    fn clear(&mut self) {
        for index in 0..PAGE {
            unsafe { write_volatile(self.virtual_address.add(index), 0) };
        }
    }
}

impl Drop for Dma {
    fn drop(&mut self) {
        lumen::dma_free(self.virtual_address);
    }
}

#[repr(C)]
struct Command {
    words: [u32; 16],
}

impl Command {
    fn new(opcode: u8, nsid: u32, prp: usize) -> Self {
        let mut words = [0; 16];
        words[0] = u32::from(opcode);
        words[1] = nsid;
        words[6] = prp as u32;
        words[7] = (prp >> 32) as u32;
        Self { words }
    }
}

struct Queue {
    sq: Dma,
    cq: Dma,
    id: u16,
    depth: u16,
    stride: usize,
    tail: u16,
    head: u16,
    cid: u16,
    phase: u16,
}

impl Queue {
    fn new(id: u16, depth: u16, stride: usize) -> Result<Self, Error> {
        let mut sq = Dma::new()?;
        let mut cq = Dma::new()?;
        sq.clear();
        cq.clear();
        Ok(Self {
            sq,
            cq,
            id,
            depth,
            stride,
            tail: 0,
            head: 0,
            cid: 0,
            phase: 1,
        })
    }

    fn submit(&mut self, mut cmd: Command) -> Result<(), Error> {
        let cid = self.cid;
        self.cid = self.cid.wrapping_add(1);
        cmd.words[0] |= u32::from(cid) << 16;
        let submission = self.sq.virtual_address.cast::<Command>();
        unsafe { write_volatile(submission.add(self.tail as usize), cmd) };
        self.tail = (self.tail + 1) % self.depth;
        let doorbell = DOORBELLS + 2 * usize::from(self.id) * self.stride;
        fence(Ordering::Release);
        reg_write(doorbell, u32::from(self.tail));

        let completion = unsafe { self.cq.virtual_address.add(usize::from(self.head) * 16) };
        let mut status = 0;
        wait(COMMAND_TIMEOUT, || {
            if reg_read(CSTS) & 2 != 0 {
                return Err(Error::Fatal);
            }
            status = unsafe { read_volatile(completion.add(14).cast::<u16>()) };
            Ok(status & 1 == self.phase)
        })?;
        fence(Ordering::Acquire);
        let sq_head = unsafe { read_volatile(completion.add(8).cast::<u16>()) };
        let sq_id = unsafe { read_volatile(completion.add(10).cast::<u16>()) };
        let completed_cid = unsafe { read_volatile(completion.add(12).cast::<u16>()) };
        if sq_head >= self.depth || sq_id != self.id || completed_cid != cid {
            return Err(Error::Completion);
        }
        self.head += 1;
        if self.head == self.depth {
            self.head = 0;
            self.phase ^= 1;
        }
        reg_write(doorbell + self.stride, u32::from(self.head));
        if status & 0x0ffe != 0 {
            return Err(Error::Command(status >> 1));
        }
        Ok(())
    }
}

fn wait(timeout: usize, mut done: impl FnMut() -> Result<bool, Error>) -> Result<(), Error> {
    let start = lumen::clock_ms();
    loop {
        if done()? {
            return Ok(());
        }
        if lumen::clock_ms().wrapping_sub(start) >= timeout {
            return Err(Error::Timeout);
        }
        lumen::yield_now();
    }
}

pub struct Namespace {
    pub id: u32,
    pub blocks: u64,
    pub block_size: usize,
}

// Pages are released only after the device can no longer perform DMA.
pub struct Controller {
    timeout: usize,
    admin: Option<Queue>,
    io: Option<Queue>,
    buffer: Option<Dma>,
    enabled: bool,
}

impl Controller {
    pub fn new() -> Self {
        Self {
            timeout: 500,
            admin: None,
            io: None,
            buffer: None,
            enabled: false,
        }
    }

    fn ready(&self, expected: bool) -> Result<(), Error> {
        wait(self.timeout, || {
            let status = reg_read(CSTS);
            if expected && status & 2 != 0 {
                return Err(Error::Fatal);
            }
            Ok((status & 1 != 0) == expected)
        })
    }

    pub fn init(&mut self, bar: usize) -> Result<Namespace, Error> {
        if !lumen::map_mmio(MMIO, bar) {
            return Err(Error::Mapping);
        }
        let cap_low = reg_read(CAP);
        let cap_high = reg_read(CAP + 4);
        let version = reg_read(VS);
        let depth = ((cap_low & 0xffff) + 1).min(64) as u16;
        if depth < 2
            || (cap_high & (1 << 5)) == 0
            || (cap_high >> 16) & 0xf != 0
            || version < 0x0001_0100
        {
            return Err(Error::Unsupported);
        }
        self.timeout = (((cap_low >> 24) as usize).max(1)) * 500;
        let stride = 4 << (cap_high & 0xf);
        let mut previous_page = 0;
        for index in 0..4 {
            let page = (DOORBELLS + index * stride) & !(PAGE - 1);
            if page != previous_page {
                let physical = bar.checked_add(page).ok_or(Error::Mapping)?;
                if !lumen::map_mmio(MMIO + page, physical) {
                    return Err(Error::Mapping);
                }
                previous_page = page;
            }
        }

        reg_write(CC, reg_read(CC) & !1);
        self.ready(false)?;
        reg_write(INTMS, u32::MAX);
        self.admin = Some(Queue::new(0, depth, stride)?);
        self.io = Some(Queue::new(1, depth, stride)?);
        self.buffer = Some(Dma::new()?);
        let admin = self.admin.as_mut().unwrap();
        let size = u32::from(depth - 1);
        reg_write(AQA, size | (size << 16));
        reg_write_address(ASQ, admin.sq.physical);
        reg_write_address(ACQ, admin.cq.physical);
        self.enabled = true;
        reg_write(CC, 1 | (6 << 16) | (4 << 20));
        self.ready(true)?;

        self.identify(0, 1)?;
        self.print_identity();
        let namespace = self.first_namespace()?;

        let io = self.io.as_ref().unwrap();
        let queue_size = 1 | (size << 16);
        let mut create_cq = Command::new(5, 0, io.cq.physical);
        create_cq.words[10] = queue_size;
        create_cq.words[11] = 1; // Physically contiguous, interrupts disabled.
        let mut create_sq = Command::new(1, 0, io.sq.physical);
        create_sq.words[10] = queue_size;
        create_sq.words[11] = 1 | (1 << 16); // Physically contiguous, CQ 1.
        let admin = self.admin.as_mut().unwrap();
        let mut queues = Command::new(9, 0, 0);
        queues.words[10] = 7; // Set Features: request one SQ and one CQ.
        admin.submit(queues)?;
        admin.submit(create_cq)?;
        admin.submit(create_sq)?;
        Ok(namespace)
    }

    fn identify(&mut self, nsid: u32, selector: u32) -> Result<(), Error> {
        let buffer = self.buffer.as_mut().unwrap();
        buffer.clear();
        let mut cmd = Command::new(6, nsid, buffer.physical);
        cmd.words[10] = selector;
        self.admin.as_mut().unwrap().submit(cmd)
    }

    fn print_identity(&self) {
        let buffer = self.buffer.as_ref().unwrap();
        for (label, offset, length) in [
            (&b"NVMED: Serial: "[..], 4, 20),
            (&b"NVMED: Model: "[..], 24, 40),
            (&b"NVMED: Firmware: "[..], 64, 8),
        ] {
            let mut field = [0; 40];
            buffer.bytes(offset, &mut field[..length]);
            let end = field[..length]
                .iter()
                .rposition(|byte| *byte != b' ' && *byte != 0);
            lumen::log(label);
            if let Some(end) = end {
                lumen::log(&field[..=end]);
            }
            lumen::log(b"\n");
        }
    }

    fn first_namespace(&mut self) -> Result<Namespace, Error> {
        self.identify(0, 2)?;
        let mut id = [0; 4];
        self.buffer.as_ref().unwrap().bytes(0, &mut id);
        let id = u32::from_le_bytes(id);
        if id == 0 {
            return Err(Error::Unsupported);
        }

        self.identify(id, 0)?;
        let buffer = self.buffer.as_ref().unwrap();
        let mut header = [0; 30];
        buffer.bytes(0, &mut header);
        let blocks = u64::from_le_bytes(header[..8].try_into().unwrap());
        let format = usize::from((header[26] & 0xf) | ((header[26] & 0x60) >> 1));
        if blocks == 0 || format > usize::from(header[25]) || header[29] & 7 != 0 {
            return Err(Error::Unsupported);
        }
        let mut lbaf = [0; 4];
        buffer.bytes(128 + format * 4, &mut lbaf);
        if lbaf[..2] != [0, 0] || !(9..=12).contains(&lbaf[2]) {
            return Err(Error::Unsupported);
        }
        Ok(Namespace {
            id,
            blocks,
            block_size: 1 << lbaf[2],
        })
    }

    pub fn io(
        &mut self,
        opcode: u8,
        ns: &Namespace,
        lba: u64,
        data: &mut [u8],
    ) -> Result<(), Error> {
        let buffer = self.buffer.as_mut().unwrap();
        let prp = if opcode == 0 { 0 } else { buffer.physical };
        if opcode == 1 {
            buffer.write(data);
        }
        let mut cmd = Command::new(opcode, ns.id, prp);
        cmd.words[10] = lba as u32;
        cmd.words[11] = (lba >> 32) as u32;
        self.io.as_mut().unwrap().submit(cmd)?;
        if opcode == 2 {
            buffer.bytes(0, data);
        }
        Ok(())
    }

    pub fn close(self, orderly: bool) -> Result<(), Error> {
        if self.enabled {
            let shutdown = if orderly {
                reg_write(CC, reg_read(CC) | (1 << 14));
                wait(COMMAND_TIMEOUT, || {
                    let status = reg_read(CSTS);
                    if status & 2 != 0 {
                        return Err(Error::Fatal);
                    }
                    Ok((status >> 2) & 3 == 2)
                })
            } else {
                Ok(())
            };
            reg_write(CC, reg_read(CC) & !1);
            if let Err(error) = self.ready(false) {
                // The device may still DMA. Keep its claim and backing pages pinned.
                core::mem::forget(self);
                return Err(error);
            }
            lumen::release_nvme();
            return shutdown;
        }
        lumen::release_nvme();
        Ok(())
    }
}
