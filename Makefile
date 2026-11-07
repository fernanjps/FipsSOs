# FipSOs - Makefile (Polyglot x86_64 Limine Framekernel: ASM + C11 + Rust + C++20)

TARGET         := x86_64-unknown-none
PROFILE        ?= debug
CARGO_FLAGS    := $(if $(filter release,$(PROFILE)),--release,)
KERNEL_ELF     := target/$(TARGET)/$(PROFILE)/fipsos-kernel
NPU_INFER_ELF  := target/$(TARGET)/$(PROFILE)/npu_infer

BUILD_DIR      := build
AVATAR_CPP_ELF := $(BUILD_DIR)/avatar_cpp.elf
ISO_ROOT       := $(BUILD_DIR)/iso_root
ISO_IMAGE      := $(BUILD_DIR)/fipsos-x86_64.iso
INITRAMFS_ROOT := $(BUILD_DIR)/initramfs_root
INITRAMFS_TAR  := $(BUILD_DIR)/initramfs.tar
LIMINE_DIR     := $(BUILD_DIR)/limine
LIMINE_REPO    := https://github.com/limine-bootloader/limine.git
LIMINE_TAG     := v8.x-binary

CXX            := g++
CXXFLAGS       := -std=c++20 -ffreestanding -fno-exceptions -fno-rtti -fno-stack-protector -mno-red-zone -msse2 -mfpmath=sse -fno-pic -fno-pie -no-pie -nostdlib -O2 -Wall -Wextra -Werror

QEMU           := qemu-system-x86_64
QEMU_FLAGS     := -M q35 -cpu max -smp 2 -m 512M -serial stdio -no-reboot -no-shutdown

.PHONY: all build cpp_user initramfs iso run clean

all: build cpp_user

build:
	cargo build --workspace --target $(TARGET) $(CARGO_FLAGS)

cpp_user:
	mkdir -p $(BUILD_DIR)
	$(CXX) $(CXXFLAGS) -T cpp_userland/linker_cpp_user.ld -Wl,--gc-sections,--strip-all,--build-id=none -Wl,-z,max-page-size=0x1000 cpp_userland/avatar_cpp.cpp -o $(AVATAR_CPP_ELF)

initramfs: build cpp_user
	rm -rf $(INITRAMFS_ROOT)
	mkdir -p $(INITRAMFS_ROOT)/etc $(INITRAMFS_ROOT)/models $(INITRAMFS_ROOT)/bin
	printf "[MOTD] Bienvenido a FipSOs v1.1 (FPU/SSE2 FXSAVE64 + C11 FP32 GEMM + Rust + C++20 Phong 3D Engine)\n" > $(INITRAMFS_ROOT)/etc/motd.txt
	printf "FIPSOS_AI_MODEL_AVATAR_V1|ARCH=NPU_INT8_TENSOR|SHARD=ZERO_COPY_DMA|DIM=512x512\n" > $(INITRAMFS_ROOT)/models/avatar_v1.bin
	cp -v $(NPU_INFER_ELF) $(INITRAMFS_ROOT)/bin/npu_infer.elf
	cp -v $(AVATAR_CPP_ELF) $(INITRAMFS_ROOT)/bin/avatar_cpp.elf
	tar --format=ustar -cf $(INITRAMFS_TAR) -C $(INITRAMFS_ROOT) etc/motd.txt models/avatar_v1.bin bin/npu_infer.elf bin/avatar_cpp.elf

$(LIMINE_DIR)/limine:
	mkdir -p $(BUILD_DIR)
	rm -rf $(LIMINE_DIR)
	git clone $(LIMINE_REPO) --branch=$(LIMINE_TAG) --depth=1 $(LIMINE_DIR)
	$(MAKE) -C $(LIMINE_DIR)

iso: build initramfs $(LIMINE_DIR)/limine
	rm -rf $(ISO_ROOT)
	mkdir -p $(ISO_ROOT)/boot/limine
	mkdir -p $(ISO_ROOT)/EFI/BOOT
	cp -v $(KERNEL_ELF) $(ISO_ROOT)/boot/fipsos-kernel
	cp -v $(INITRAMFS_TAR) $(ISO_ROOT)/boot/initramfs.tar
	cp -v limine.conf $(ISO_ROOT)/boot/limine/
	cp -v $(LIMINE_DIR)/limine-bios.sys $(ISO_ROOT)/boot/limine/
	cp -v $(LIMINE_DIR)/limine-bios-cd.bin $(ISO_ROOT)/boot/limine/
	cp -v $(LIMINE_DIR)/limine-uefi-cd.bin $(ISO_ROOT)/boot/limine/
	cp -v $(LIMINE_DIR)/BOOTX64.EFI $(ISO_ROOT)/EFI/BOOT/
	xorriso -as mkisofs -b boot/limine/limine-bios-cd.bin \
		-no-emul-boot -boot-load-size 4 -boot-info-table \
		--efi-boot boot/limine/limine-uefi-cd.bin \
		-efi-boot-part --efi-boot-image --protective-msdos-label \
		$(ISO_ROOT) -o $(ISO_IMAGE)
	$(LIMINE_DIR)/limine bios-install $(ISO_IMAGE)

run: iso
	$(QEMU) $(QEMU_FLAGS) -cdrom $(ISO_IMAGE)

clean:
	cargo clean
	rm -rf $(BUILD_DIR)
