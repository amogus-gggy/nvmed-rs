# nvmed

A freestanding Rust NVMe block service for Nyvela. It discovers a PCI NVMe
controller, prints its identity, and serves the first active namespace over
Nyvela IPC. The driver uses polled admin and I/O queues, 4 KiB DMA pages, and
one outstanding command at a time.

## Build and run

```sh
rustup target add x86_64-unknown-none
make
make test
```

Building requires Rust 1.85 or later and GNU objcopy. The build produces
`build/nvmed.bin` and `build/nvmed.elf`. The flat image starts at Nyvela's
program load address, `0x500000`.

Use the Nyvela kernel with the NVMe claim and clock syscalls and the
4112-byte IPC limit. In `nyvela.conf`, set `NVMED_DIR` to this checkout and
`NVMED_BIN_NAME` to `nvmed`. Nyvela embeds the build as `/bin/nvmed`.
Launch it via `sys_exec("/bin/nvmed")` from a launcher thread, leaving another
thread free to issue requests. `sys_exec` waits for the service to exit.
The driver prints its TID once it is ready. With the companion Nyvela changes,
`make run` provides a writable QEMU test namespace backed by `build/nvme.img`.

The service supports one controller and one namespace formatted for 512 to
4096-byte logical blocks without metadata or protection information. The
controller needs a firmware-assigned BAR0, 4 KiB page support, and the NVM
command set. Callers in Nyvela's trusted userspace can issue raw writes and
stop the service. A successful flush completes persistence of preceding
writes. A stop requests normal controller shutdown and returns the DMA pages
once the controller is disabled.

## IPC protocol

Send requests to the driver's TID using `sys_ipc_send`. Use
`sys_ipc_poll` to receive replies and check that the sender is the driver's
TID. All integers are little-endian. LBA values count logical blocks from
zero. The client can use the tag to correlate a reply with its request.

| Request offset | Bytes | Field |
| --- | ---: | --- |
| 0 | 1 | Opcode |
| 1 | 3 | Reserved, zero |
| 4 | 4 | Tag |
| 8 | 8 | LBA, zero for Info, Flush, and Stop |
| 16 | Variable | Exactly one logical block for Write, empty for other requests |

| Opcode | Operation | Success payload |
| ---: | --- | --- |
| 0 | Info | Namespace ID (`u32`), block size in bytes (`u32`), block count (`u64`) |
| 1 | Read | One logical block |
| 2 | Write | Empty |
| 3 | Flush | Empty |
| 4 | Stop | Empty |

| Reply offset | Bytes | Field |
| --- | ---: | --- |
| 0 | 4 | Signed status. `0` success, `-1` malformed request, `-2` LBA out of range, `-3` device error |
| 4 | 4 | Echoed tag, or zero if the malformed request omitted it |
| 8 | 2 | NVMe completion status with the phase bit removed, or zero |
| 10 | 2 | Reserved, zero |
| 12 | 4 | Payload length in bytes |
| 16 | Variable | Payload |
