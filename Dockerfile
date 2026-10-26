FROM debian:testing

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        moreutils \
        procps \
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

RUN mkdir /workspace && echo 'PATH=/workspace/.local/bin:$PATH' >> /workspace/.bashrc

WORKDIR /workspace

ENV HOME=/workspace \
    SHELL=/bin/bash \
    BASH_ENV=/workspace/.bashrc

ENTRYPOINT ["/entrypoint.sh"]
