# 32-bit Wine for running the Borland C++Builder 6 command-line tools.
# The toolchain itself is never baked into the image: build.sh bind-mounts
# a licensed install read-only at /bcb.
FROM debian:trixie-slim
RUN dpkg --add-architecture i386 \
 && apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends wine wine32:i386 \
 && rm -rf /var/lib/apt/lists/*
ARG UID=1000
RUN useradd -m -u "$UID" builder
USER builder
ENV WINEDEBUG=-all WINEPREFIX=/home/builder/.wine WINEPATH='Z:\bcb\Bin'
RUN wineboot --init; wineserver -w
WORKDIR /work
