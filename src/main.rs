#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod protocol;

#[cfg(not(test))]
mod nvme;
#[cfg(not(test))]
mod lumen;

#[cfg(not(test))]
mod driver {
    use crate::{
        nvme::{self, Controller, Error, Namespace},
        lumen,
        protocol::*,
    };
    use core::{arch::global_asm, panic::PanicInfo, ptr};

    global_asm!(
        ".section .text._start,\"ax\",@progbits",
        ".global _start",
        "_start:",
        "cld",
        // The flat loader supplies no ELF relocation support. Apply the static
        // PIE's R_X86_64_RELATIVE entries before Rust code can use the GOT.
        "lea rdx, [rip + _start]",
        "lea rsi, [rip + relocations_start]",
        "lea rcx, [rip + relocations_end]",
        "2:",
        "cmp rsi, rcx",
        "je 3f",
        "cmp qword ptr [rsi + 8], 8",
        "jne 4f",
        "mov rax, [rsi]",
        "mov rdi, [rsi + 16]",
        "add rdi, rdx",
        "mov [rdx + rax], rdi",
        "add rsi, 24",
        "jmp 2b",
        "3:",
        "and rsp, -16",
        "call nvmed_main",
        "4:",
        "ud2",
    );

    static mut MESSAGE_BUFFER: [u8; MESSAGE] = [0; MESSAGE];

    const INVALID: i32 = -1;
    const RANGE: i32 = -2;
    const DEVICE: i32 = -3;

    fn reply(message: &mut [u8], tag: u32, status: i32, detail: u16, length: usize) {
        message[..HEADER].fill(0);
        message[..4].copy_from_slice(&status.to_le_bytes());
        message[4..8].copy_from_slice(&tag.to_le_bytes());
        message[8..10].copy_from_slice(&detail.to_le_bytes());
        message[12..16].copy_from_slice(&(length as u32).to_le_bytes());
    }

    fn stop(controller: Controller, orderly: bool) -> bool {
        match controller.close(orderly) {
            Ok(()) => true,
            Err(error) => {
                lumen::log(error.message());
                false
            }
        }
    }

    fn serve(mut controller: Controller, ns: Namespace) -> usize {
        // This process has one thread and one request in flight.
        let message = unsafe { &mut *ptr::addr_of_mut!(MESSAGE_BUFFER) };
        lumen::log(b"NVMED: ready, TID ");
        let mut digits = [0; 3 * core::mem::size_of::<usize>()];
        let mut tid = lumen::tid();
        let mut index = digits.len();
        loop {
            index -= 1;
            digits[index] = b'0' + (tid % 10) as u8;
            tid /= 10;
            if tid == 0 {
                break;
            }
        }
        lumen::log(&digits[index..]);
        lumen::log(b"\n");

        loop {
            let mut sender = 0;
            let received = lumen::poll(message, &mut sender);
            if received == 0 {
                lumen::yield_now();
                continue;
            }
            let Some(request) = Request::decode(&message[..received], ns.block_size) else {
                let tag = if received >= 8 {
                    u32::from_le_bytes(message[4..8].try_into().unwrap())
                } else {
                    0
                };
                reply(message, tag, INVALID, 0, 0);
                lumen::send(sender, &message[..HEADER]);
                continue;
            };

            let (status, detail, length, fatal) = match request.opcode {
                INFO => {
                    message[16..20].copy_from_slice(&ns.id.to_le_bytes());
                    message[20..24].copy_from_slice(&(ns.block_size as u32).to_le_bytes());
                    message[24..32].copy_from_slice(&ns.blocks.to_le_bytes());
                    (0, 0, 16, false)
                }
                STOP => {
                    let code = if stop(controller, true) { 0 } else { DEVICE };
                    reply(message, request.tag, code, 0, 0);
                    lumen::send(sender, &message[..HEADER]);
                    return usize::from(code != 0);
                }
                READ | WRITE if request.lba >= ns.blocks => (RANGE, 0, 0, false),
                opcode => {
                    let command = match opcode {
                        READ => 2,
                        WRITE => 1,
                        _ => 0,
                    };
                    let data = &mut message[HEADER..HEADER + ns.block_size];
                    match controller.io(command, &ns, request.lba, data) {
                        Ok(()) => (0, 0, if opcode == READ { ns.block_size } else { 0 }, false),
                        Err(Error::Command(status)) => (DEVICE, status, 0, false),
                        Err(error) => {
                            lumen::log(error.message());
                            (DEVICE, 0, 0, true)
                        }
                    }
                }
            };
            reply(message, request.tag, status, detail, length);
            lumen::send(sender, &message[..HEADER + length]);
            if fatal {
                stop(controller, false);
                return 1;
            }
        }
    }

    #[no_mangle]
    extern "C" fn nvmed_main() -> ! {
        lumen::log(b"NVMED: start\n");
        let Some((bdf, bar)) = nvme::discover() else {
            lumen::log(b"NVMED: no NVMe PCI device with a usable BAR0\n");
            lumen::exit(1);
        };
        lumen::log(b"NVMED: discovered NVMe device\n");
        if !lumen::claim_nvme(bdf) {
            lumen::log(b"NVMED: controller busy or kernel NVMe claim unavailable\n");
            lumen::exit(1);
        }
        lumen::log(b"NVMED: claimed controller\n");
        let mut controller = Controller::new();
        let status = match controller.init(bar) {
            Ok(namespace) => {
                lumen::log(b"NVMED: controller initialized\n");
                serve(controller, namespace)
            }
            Err(error) => {
                lumen::log(error.message());
                stop(controller, false);
                1
            }
        };
        lumen::exit(status);
    }

    #[panic_handler]
    fn panic(_: &PanicInfo<'_>) -> ! {
        lumen::log(b"NVMED: panic, controller may require reset\n");
        lumen::exit(1);
    }
}
