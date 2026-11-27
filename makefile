CARGO ?= cargo
OBJCOPY ?= objcopy
ARCH := x86_64-unknown-none
BUILD := build
RUSTFLAGS := -C relocation-model=static -C link-arg=-Tlinker.ld -C link-arg=--no-pie

.PHONY: all clean test
all: $(BUILD)/nvmed.elf $(BUILD)/nvmed.bin

$(BUILD)/nvmed.elf: $(wildcard src/*.rs) Cargo.toml Cargo.lock linker.ld makefile
	RUSTFLAGS="$(RUSTFLAGS)" $(CARGO) build --locked --release --target $(ARCH) --target-dir $(BUILD)/cargo
	cp $(BUILD)/cargo/$(ARCH)/release/nvmed $@

$(BUILD)/nvmed.bin: $(BUILD)/nvmed.elf
	$(OBJCOPY) -O binary --set-section-flags .bss=alloc,load,contents $< $@
	test "$$(wc -c < $@)" -le 65536

test:
	$(CARGO) test --locked --target-dir $(BUILD)/cargo

clean:
	rm -rf $(BUILD)
