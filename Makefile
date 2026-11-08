# FipSOs - Makefile (Polyglot x86_64 Limine Framekernel: ASM + C11 + Rust + C++20)

TARGET         := x86_64-unknown-none
PROFILE        ?= debug
CARGO_FLAGS    := $(if $(filter release,$(PROFILE)),--release,)
KERNEL_ELF     := target/$(TARGET)/$(PROFILE)/fipsos-kernel
NPU_INFER_ELF  := target/$(TARGET)/$(PROFILE)/npu_infer

BUILD_DIR      := build
AVATAR_CPP_ELF := $(BUILD_DIR)/avatar_cpp.elf
DESKTOP_UI_ELF := $(BUILD_DIR)/desktop_ui.elf
C_STD_APP_ELF  := $(BUILD_DIR)/c_std_app.elf
PKG_ELF        := $(BUILD_DIR)/pkg.elf
FIPSOS_SH_ELF  := $(BUILD_DIR)/fipsos_sh.elf
KILO_ELF       := $(BUILD_DIR)/kilo.elf
ISO_ROOT       := $(BUILD_DIR)/iso_root
ISO_IMAGE      := $(BUILD_DIR)/fipsos-x86_64.iso
DISK_IMAGE     := $(BUILD_DIR)/fipsos_disk.img
INITRAMFS_ROOT := $(BUILD_DIR)/initramfs_root
INITRAMFS_TAR  := $(BUILD_DIR)/initramfs.tar
LIMINE_DIR     := $(BUILD_DIR)/limine
LIMINE_REPO    := https://github.com/limine-bootloader/limine.git
LIMINE_TAG     := v8.x-binary

CC             := gcc
CFLAGS         := -static -O2 -fno-stack-protector -no-pie -Wall -Wextra

CXX            := g++
CXXFLAGS       := -std=c++20 -ffreestanding -fno-exceptions -fno-rtti -fno-stack-protector -mno-red-zone -msse2 -mfpmath=sse -fno-pic -fno-pie -no-pie -nostdlib -O2 -Wall -Wextra -Werror

QEMU           := qemu-system-x86_64
QEMU_FLAGS     := -M q35 -cpu max -smp 2 -m 512M -serial stdio -no-reboot -no-shutdown -drive id=satadisk,file=$(DISK_IMAGE),format=raw,if=none -device ide-hd,drive=satadisk,bus=ide.0 -netdev user,id=net0 -device e1000e,netdev=net0

.PHONY: all build cpp_user c_user initramfs iso run clean

all: build cpp_user c_user

build:
	cargo build --workspace --target $(TARGET) $(CARGO_FLAGS)

$(DISK_IMAGE):
	mkdir -p $(BUILD_DIR)
	test -f $(DISK_IMAGE) || truncate -s 16M $(DISK_IMAGE)

c_user:
	mkdir -p $(BUILD_DIR)
	$(CC) $(CFLAGS) c_std_userland/posix_test.c -o $(C_STD_APP_ELF)
	$(CC) $(CFLAGS) c_std_userland/pkg.c -o $(PKG_ELF)
	$(CC) $(CFLAGS) c_std_userland/fipsos_sh.c -o $(FIPSOS_SH_ELF)
	$(CC) $(CFLAGS) c_std_userland/kilo_editor.c -o $(KILO_ELF)

cpp_user:
	mkdir -p $(BUILD_DIR)
	$(CXX) $(CXXFLAGS) -T cpp_userland/linker_cpp_user.ld -Wl,--gc-sections,--strip-all,--build-id=none -Wl,-z,max-page-size=0x1000 cpp_userland/avatar_cpp.cpp -o $(AVATAR_CPP_ELF)
	$(CXX) $(CXXFLAGS) -T cpp_userland/linker_desktop_user.ld -Wl,--gc-sections,--strip-all,--build-id=none -Wl,-z,max-page-size=0x1000 cpp_userland/desktop_ui.cpp -o $(DESKTOP_UI_ELF)

initramfs: build cpp_user c_user
	rm -rf $(INITRAMFS_ROOT)
	mkdir -p $(INITRAMFS_ROOT)/etc $(INITRAMFS_ROOT)/models $(INITRAMFS_ROOT)/bin
	printf "[MOTD] Bienvenido a FipSOs v1.9.3 (Linux C stdlib + Interactive Shell + Kilo Editor + SATA + WiFi)\n" > $(INITRAMFS_ROOT)/etc/motd.txt
	printf "FIPSOS_AI_MODEL_AVATAR_V1|ARCH=NPU_INT8_TENSOR|SHARD=ZERO_COPY_DMA|DIM=512x512\n" > $(INITRAMFS_ROOT)/models/avatar_v1.bin
	cp -v $(NPU_INFER_ELF) $(INITRAMFS_ROOT)/bin/npu_infer.elf
	cp -v $(AVATAR_CPP_ELF) $(INITRAMFS_ROOT)/bin/avatar_cpp.elf
	cp -v $(DESKTOP_UI_ELF) $(INITRAMFS_ROOT)/bin/desktop_ui.elf
	cp -v $(C_STD_APP_ELF) $(INITRAMFS_ROOT)/bin/c_std_app.elf
	cp -v $(PKG_ELF) $(INITRAMFS_ROOT)/bin/pkg.elf
	cp -v $(PKG_ELF) $(INITRAMFS_ROOT)/bin/pkg
	cp -v $(FIPSOS_SH_ELF) $(INITRAMFS_ROOT)/bin/fipsos_sh.elf
	cp -v $(FIPSOS_SH_ELF) $(INITRAMFS_ROOT)/bin/sh
	cp -v $(KILO_ELF) $(INITRAMFS_ROOT)/bin/kilo.elf
	cp -v $(KILO_ELF) $(INITRAMFS_ROOT)/bin/edit
	cp -v build/busybox_bin/busybox $(INITRAMFS_ROOT)/bin/busybox
	tar --format=ustar -cf $(INITRAMFS_TAR) -C $(INITRAMFS_ROOT) etc/motd.txt models/avatar_v1.bin bin/npu_infer.elf bin/avatar_cpp.elf bin/desktop_ui.elf bin/c_std_app.elf bin/pkg.elf bin/pkg bin/fipsos_sh.elf bin/sh bin/kilo.elf bin/edit bin/busybox

$(LIMINE_DIR)/limine:
	mkdir -p $(BUILD_DIR)
	rm -rf $(LIMINE_DIR)
	git clone $(LIMINE_REPO) --branch=$(LIMINE_TAG) --depth=1 $(LIMINE_DIR)
	$(MAKE) -C $(LIMINE_DIR)

iso: build initramfs $(DISK_IMAGE) $(LIMINE_DIR)/limine
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

run: iso $(DISK_IMAGE)
	$(QEMU) $(QEMU_FLAGS) -cdrom $(ISO_IMAGE)

clean:
	cargo clean
	rm -rf $(BUILD_DIR)
