# Builds the .deb on Debian 13 (trixie), which has GTK 4.18 and
# libadwaita 1.7 (the app needs GTK >= 4.14 and libadwaita >= 1.6).
# Used by ../../build.sh.
FROM rust:1-trixie

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      libgtk-4-dev libadwaita-1-dev pkg-config dpkg-dev \
 && rm -rf /var/lib/apt/lists/*
