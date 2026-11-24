# Inno Setup compiler (ISCC) under wine, for the Windows installer.
# Used by ../../build.sh after build-windows.sh has staged the application.
FROM debian:trixie-slim

# wine32 is needed even for 64-bit programs: wine sets up a WoW64 prefix.
RUN dpkg --add-architecture i386 \
 && apt-get update \
 && apt-get install -y --no-install-recommends wine wine64 wine32:i386 xvfb xauth ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*

ENV WINEPREFIX=/opt/wine WINEDEBUG=-all WINEDLLOVERRIDES="mscoree,mshtml="
ARG INNO=7.1.0
RUN curl -fsSL -o /tmp/innosetup.exe \
      "https://github.com/jrsoftware/issrc/releases/download/is-$(echo $INNO | tr . _)/innosetup-${INNO}-x64.exe" \
 && xvfb-run -a wine /tmp/innosetup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /ALLUSERS '/DIR=C:\InnoSetup' \
 && wineserver -w \
 && rm /tmp/innosetup.exe \
 && test -f "$WINEPREFIX/drive_c/InnoSetup/ISCC.exe" \
 && chmod -R a+rwX "$WINEPREFIX"
