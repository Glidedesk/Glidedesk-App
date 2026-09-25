# glidedesk-lab — real end-to-end tests on Linux without hardware: virtual X
# screens (Xvfb, several monitors via RandR), real agents talking QUIC over
# dummy network interfaces, and the settings UI served for browser QA.
FROM glidedesk-builder:latest
RUN apt-get update \
 && apt-get install -y --no-install-recommends xvfb x11-xserver-utils xdotool iproute2 procps \
 && rm -rf /var/lib/apt/lists/*
