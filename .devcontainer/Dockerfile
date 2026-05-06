FROM mcr.microsoft.com/devcontainers/rust:1-1-bookworm

RUN DEBIAN_FRONTEND=noninteractive apt-get update && apt-get install -y --no-install-recommends \
    clang jq curl tar xz-utils \
    pipewire-alsa libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev \
    libxi6 libxrandr-dev libxcursor-dev \
    libegl1-mesa libgl1-mesa-dri libxcb-xfixes0-dev mesa-vulkan-drivers
RUN MOLD_VERSION=$(curl -s https://api.github.com/repos/rui314/mold/releases/latest | jq -r .tag_name | sed 's/^v//') && \
    curl -L https://github.com/rui314/mold/releases/download/v${MOLD_VERSION}/mold-${MOLD_VERSION}-x86_64-linux.tar.gz \
    | tar xz && \
    cp mold-${MOLD_VERSION}-x86_64-linux/bin/mold /usr/local/bin/ && \
    ln -s /usr/local/bin/mold /usr/local/bin/ld.mold && \
    rm -rf mold-${MOLD_VERSION}-x86_64-linux
USER vscode
RUN rustup default nightly \
    && rustup target add wasm32-unknown-unknown \
    && rustup component add rustfmt rustc-codegen-cranelift-preview clippy
#    && cargo install trunk
