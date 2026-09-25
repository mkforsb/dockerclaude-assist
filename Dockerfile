FROM debian:testing

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        build-essential \
        git \
        python3-pip \
        pipx \
    && rm -rf /var/lib/apt/lists/*

COPY installers /installers

COPY entrypoint.sh /entrypoint.sh

RUN chmod 0755 /entrypoint.sh

RUN mkdir /workspace

WORKDIR /workspace

ENV HOME=/workspace \
    PATH=/workspace/.local/bin:/usr/local/bin:/usr/bin:/bin

ENTRYPOINT ["/entrypoint.sh"]
