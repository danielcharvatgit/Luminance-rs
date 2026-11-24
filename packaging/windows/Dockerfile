# Cross-compiles Luminance-rs for 64-bit Windows: llvm-mingw (UCRT) as C
# toolchain and linker, prebuilt GTK 4 / libadwaita from MSYS2's CLANG64
# repository. The installer is made by iscc.Dockerfile. Used by ../../build.sh.
FROM rust:1-trixie

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      pkg-config zstd xz-utils python3 zip librsvg2-bin libglib2.0-bin \
      librsvg2-common libgdk-pixbuf2.0-bin \
 && rm -rf /var/lib/apt/lists/*

ARG LLVM_MINGW=20261020
RUN curl -fsSL "https://github.com/mstorsjo/llvm-mingw/releases/download/${LLVM_MINGW}/llvm-mingw-${LLVM_MINGW}-ucrt-ubuntu-22.04-x86_64.tar.xz" \
      | tar -xJ -C /opt \
 && mv /opt/llvm-mingw-* /opt/llvm-mingw
ENV PATH=/opt/llvm-mingw/bin:$PATH

RUN rustup target add x86_64-pc-windows-gnullvm

COPY msys2-fetch.py /usr/local/bin/msys2-fetch.py
RUN python3 /usr/local/bin/msys2-fetch.py /opt/msys2 \
      gtk4 libadwaita adwaita-icon-theme hicolor-icon-theme librsvg

ENV CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER=x86_64-w64-mingw32-clang \
    CC_x86_64_pc_windows_gnullvm=x86_64-w64-mingw32-clang \
    PKG_CONFIG_ALLOW_CROSS=1 \
    PKG_CONFIG_SYSROOT_DIR=/opt/msys2 \
    PKG_CONFIG_LIBDIR=/opt/msys2/clang64/lib/pkgconfig:/opt/msys2/clang64/share/pkgconfig \
    MSYS2_ROOT=/opt/msys2/clang64
