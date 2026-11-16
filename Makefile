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
HTTPD_ELF      := $(BUILD_DIR)/fipsos_httpd.elf
BEEP_ELF       := $(BUILD_DIR)/beep.elf
IWCTL_ELF      := $(BUILD_DIR)/iwctl_fips.elf
SANDBOX_PROBE  := $(BUILD_DIR)/sandbox_probe.elf
SAMPLE_ELF     := $(BUILD_DIR)/sample_tool.elf
BENCHMARK_ELF  := $(BUILD_DIR)/benchmark_simd.elf
DOCKER_ELF     := $(BUILD_DIR)/docker
GOOSE_MON      := $(BUILD_DIR)/goose_mon
GOOSE_AUDIT    := $(BUILD_DIR)/goose_audit
FIPS_AI        := $(BUILD_DIR)/fips_ai
LUA_ELF        := $(BUILD_DIR)/lua
TCC_ELF        := $(BUILD_DIR)/tcc
ISO_ROOT       := $(BUILD_DIR)/iso_root
ISO_IMAGE      := $(BUILD_DIR)/fipsos-x86_64.iso
USB_IMAGE      := $(BUILD_DIR)/fipsos-baremetal-usb.img
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

.PHONY: all build cpp_user c_user goose_user tcc_user initramfs iso usb run clean remote

all: build cpp_user c_user goose_user tcc_user

build:
	cargo build --workspace --target $(TARGET) $(CARGO_FLAGS)

$(DISK_IMAGE):
	mkdir -p $(BUILD_DIR)
	test -f $(DISK_IMAGE) || truncate -s 16M $(DISK_IMAGE)

c_user:
	mkdir -p $(BUILD_DIR)
	$(CC) $(CFLAGS) c_std_userland/posix_test.c -o $(C_STD_APP_ELF)
	$(CC) $(CFLAGS) c_std_userland/pkg.c -o $(PKG_ELF)
	$(CC) $(CFLAGS) c_std_userland/fipsos_sh.c c_std_userland/sh/sh_readline.c c_std_userland/sh/sh_builtins.c -o $(FIPSOS_SH_ELF)
	$(CC) $(CFLAGS) c_std_userland/kilo_editor.c -o $(KILO_ELF)
	$(CC) $(CFLAGS) -msse2 -Ic_std_userland c_std_userland/fipsos_httpd.c -o $(HTTPD_ELF)
	$(CC) $(CFLAGS) c_std_userland/beep.c -o $(BEEP_ELF)
	$(CC) $(CFLAGS) c_std_userland/iwctl_fips.c -o $(IWCTL_ELF)
	$(CC) $(CFLAGS) c_std_userland/sandbox_probe.c -o $(SANDBOX_PROBE)
	$(CC) $(CFLAGS) c_std_userland/docker_fips.c -o $(DOCKER_ELF)
	$(CC) -static -O2 -fno-builtin -no-pie -nostdlib c_std_userland/sample_tool.c -o $(SAMPLE_ELF)
	$(CC) -static -O2 -fno-builtin -msse2 -no-pie -nostdlib c_std_userland/benchmark_simd.c -o $(BENCHMARK_ELF)
	$(CC) $(CFLAGS) -Werror c_std_userland/lua/*.c -o $(LUA_ELF) -lm
	strip $(LUA_ELF)
	cp -f $(KILO_ELF) $(BUILD_DIR)/editor_kilo.elf

tcc_user:
	sed -i 's/\r$$//' tools/build_tcc.sh
	bash tools/build_tcc.sh

cpp_user:
	mkdir -p $(BUILD_DIR)
	$(CXX) $(CXXFLAGS) -T cpp_userland/linker_cpp_user.ld -Wl,--gc-sections,--strip-all,--build-id=none -Wl,-z,max-page-size=0x1000 cpp_userland/avatar_cpp.cpp -o $(AVATAR_CPP_ELF)
	$(CXX) $(CXXFLAGS) -T cpp_userland/linker_desktop_user.ld -Wl,--gc-sections,--strip-all,--build-id=none -Wl,-z,max-page-size=0x1000 cpp_userland/desktop_ui.cpp cpp_userland/ui_renderer.cpp cpp_userland/ui_wm.cpp cpp_userland/ui_explorer.cpp cpp_userland/ui_apps.cpp cpp_userland/ui_live_monitor.cpp cpp_userland/ui_terminal.cpp cpp_userland/ui_editor.cpp cpp_userland/ui_security.cpp -o $(DESKTOP_UI_ELF)

goose_userland/%.gen.c: goose_userland/%.goose
	@if command -v goose >/dev/null 2>&1; then \
		goose $< > $@; \
	else \
		touch $@; \
	fi

goose_user: goose_userland/sys_mon.gen.c goose_userland/disk_audit.gen.c goose_userland/ai_infer.gen.c
	mkdir -p $(BUILD_DIR)
	$(CC) $(CFLAGS) -Igoose_userland goose_userland/sys_mon.gen.c -o $(GOOSE_MON)
	$(CC) $(CFLAGS) -Igoose_userland goose_userland/disk_audit.gen.c -o $(GOOSE_AUDIT)
	$(CC) $(CFLAGS) -msse2 -Igoose_userland -Ic_std_userland goose_userland/ai_infer.gen.c -o $(FIPS_AI)

initramfs: build cpp_user c_user goose_user tcc_user build/busybox_bin/busybox
	rm -rf $(INITRAMFS_ROOT)
	mkdir -p $(INITRAMFS_ROOT)/etc $(INITRAMFS_ROOT)/models $(INITRAMFS_ROOT)/bin $(INITRAMFS_ROOT)/usr/include/sys $(INITRAMFS_ROOT)/usr/lib
	printf "[MOTD] Bienvenido a FipSOs v6.3 Framekernel (x86_64 SMP | Containers | FAT32 Auto-Init)\n" > $(INITRAMFS_ROOT)/etc/motd.txt
	printf "FIPSOS_AI_MODEL_AVATAR_V1|ARCH=NPU_INT8_TENSOR|SHARD=ZERO_COPY_DMA|DIM=512x512\n" > $(INITRAMFS_ROOT)/models/avatar_v1.bin
	cp -v $(NPU_INFER_ELF) $(INITRAMFS_ROOT)/bin/npu_infer.elf
	cp -v $(AVATAR_CPP_ELF) $(INITRAMFS_ROOT)/bin/avatar_cpp.elf
	cp -v $(DESKTOP_UI_ELF) $(INITRAMFS_ROOT)/bin/desktop_ui.elf
	cp -v $(C_STD_APP_ELF) $(INITRAMFS_ROOT)/bin/c_std_app.elf
	cp -v $(PKG_ELF) $(INITRAMFS_ROOT)/bin/pkg.elf
	cp -v $(FIPSOS_SH_ELF) $(INITRAMFS_ROOT)/bin/fipsos_sh.elf
	cp -v $(KILO_ELF) $(INITRAMFS_ROOT)/bin/kilo.elf
	cp -v $(HTTPD_ELF) $(INITRAMFS_ROOT)/bin/fipsos_httpd.elf
	cp -v $(BEEP_ELF) $(INITRAMFS_ROOT)/bin/beep
	cp -v $(IWCTL_ELF) $(INITRAMFS_ROOT)/bin/iwctl_fips
	cp -v $(SANDBOX_PROBE) $(INITRAMFS_ROOT)/bin/sandbox_probe
	cp -v $(SAMPLE_ELF) $(INITRAMFS_ROOT)/bin/sample_tool.elf
	cp -v $(BENCHMARK_ELF) $(INITRAMFS_ROOT)/bin/benchmark_simd.elf
	cp -v $(DOCKER_ELF) $(INITRAMFS_ROOT)/bin/docker
	cp -v $(GOOSE_MON) $(INITRAMFS_ROOT)/bin/goose_mon
	cp -v $(GOOSE_AUDIT) $(INITRAMFS_ROOT)/bin/goose_audit
	cp -v $(FIPS_AI) $(INITRAMFS_ROOT)/bin/fips_ai
	cp -v $(LUA_ELF) $(INITRAMFS_ROOT)/bin/lua
	cp -v $(TCC_ELF) $(INITRAMFS_ROOT)/bin/tcc
	cp -v c_std_userland/tcc_sysroot/include/*.h $(INITRAMFS_ROOT)/usr/include/
	cp -v c_std_userland/tcc_sysroot/include/sys/*.h $(INITRAMFS_ROOT)/usr/include/sys/
	cp -v build/tcc_sysroot/lib/crt1.o build/tcc_sysroot/lib/crti.o build/tcc_sysroot/lib/crtn.o build/tcc_sysroot/lib/libc.a build/tcc_sysroot/lib/libtcc1.a $(INITRAMFS_ROOT)/usr/lib/
	cp -v build/busybox_bin/busybox $(INITRAMFS_ROOT)/bin/busybox
	tar --format=ustar -cf $(INITRAMFS_TAR) -C $(INITRAMFS_ROOT) etc/motd.txt models/avatar_v1.bin bin/npu_infer.elf bin/avatar_cpp.elf bin/desktop_ui.elf bin/c_std_app.elf bin/pkg.elf bin/fipsos_sh.elf bin/kilo.elf bin/fipsos_httpd.elf bin/beep bin/iwctl_fips bin/sandbox_probe bin/docker bin/sample_tool.elf bin/benchmark_simd.elf bin/goose_mon bin/goose_audit bin/fips_ai bin/lua bin/tcc bin/busybox usr

build/busybox_bin/busybox:
	mkdir -p $(BUILD_DIR)/busybox_bin
	curl -fsSL https://busybox.net/downloads/binaries/1.35.0-x86_64-linux-musl/busybox -o $(BUILD_DIR)/busybox_bin/busybox
	chmod +x $(BUILD_DIR)/busybox_bin/busybox

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

usb: iso
	cp -v $(ISO_IMAGE) $(USB_IMAGE)
	@echo "======================================================================"
	@echo "  FipSOs Bare-Metal Hybrid USB Image Created: $(USB_IMAGE)"
	@echo "  Limine BIOS (MBR) + UEFI (GPT/ESP) Boot Stages Embedded"
	@echo "  To write to a real USB flash drive:"
	@echo "    sudo dd if=$(USB_IMAGE) of=/dev/sdX bs=4M status=progress conv=fdatasync"
	@echo "======================================================================"

run: iso $(DISK_IMAGE)
	$(QEMU) $(QEMU_FLAGS) -cdrom $(ISO_IMAGE)

remote:
	python3 tools/fipsos_remote.py

clean:
	cargo clean
	rm -rf $(BUILD_DIR)
