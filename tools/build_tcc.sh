#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
BUILD_DIR="$ROOT_DIR/build"
TCC_DIR="$BUILD_DIR/tinycc"
SYS_SRC="$ROOT_DIR/c_std_userland/tcc_sysroot"
SYS_OUT="$BUILD_DIR/tcc_sysroot/lib"

mkdir -p "$BUILD_DIR" "$SYS_OUT"

if [ ! -f "$TCC_DIR/configure" ]; then
    rm -rf "$TCC_DIR"
    git clone --depth=1 https://repo.or.cz/tinycc.git "$TCC_DIR"
fi

if [ ! -f "$BUILD_DIR/tcc" ] || [ ! -f "$SYS_OUT/libtcc1.a" ]; then
    cd "$TCC_DIR"
    ./configure \
        --prefix=/usr \
        --tccdir=/usr/lib \
        --crtprefix=/usr/lib \
        --sysincludepaths=/usr/include \
        --libpaths=/usr/lib \
        --tcc-switches=-static \
        --extra-cflags="-O2 -static -fno-stack-protector -no-pie" \
        --extra-ldflags="-static -no-pie"
    make clean >/dev/null 2>&1 || true
    make -j4 tcc LDFLAGS="-static -no-pie"
    make -C lib libtcc1.o stdatomic.o atomic.o builtin.o alloca.o alloca-bt.o
    ar rcs "$SYS_OUT/libtcc1.a" \
        lib/libtcc1.o lib/stdatomic.o lib/atomic.o \
        lib/builtin.o lib/alloca.o lib/alloca-bt.o
    strip tcc
    cp -f tcc "$BUILD_DIR/tcc"
fi

gcc -c "$SYS_SRC/src/crt.S" -o "$SYS_OUT/crt1.o"
gcc -c "$SYS_SRC/src/crti.S" -o "$SYS_OUT/crti.o"
gcc -c "$SYS_SRC/src/crtn.S" -o "$SYS_OUT/crtn.o"
gcc -c -Os -nostdinc -I"$SYS_SRC/include" -fno-stack-protector -ffreestanding -fno-builtin -Wall -Wextra -Werror "$SYS_SRC/src/libc_mem.c" -o "$SYS_OUT/libc_mem.o"
gcc -c -Os -nostdinc -I"$SYS_SRC/include" -fno-stack-protector -ffreestanding -fno-builtin -Wall -Wextra -Werror "$SYS_SRC/src/libc_io.c" -o "$SYS_OUT/libc_io.o"
rm -f "$SYS_OUT/libc.a"
ar rcs "$SYS_OUT/libc.a" "$SYS_OUT/libc_mem.o" "$SYS_OUT/libc_io.o"
