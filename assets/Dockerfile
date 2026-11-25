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

COPY entrypoint.sh /entrypoint.sh

RUN chmod 0755 /entrypoint.sh

RUN mkdir /workspace && echo 'PATH=/workspace/.local/bin:$PATH' >> /workspace/.bashrc

WORKDIR /workspace

# PATH is set here as well as in .bashrc so that programs started by the hc
# core (which is not a login shell) find claude and hc.
ENV HOME=/workspace \
    SHELL=/bin/bash \
    BASH_ENV=/workspace/.bashrc \
    PATH=/workspace/.local/bin:$PATH

# `dockerclaude build-image` passes a new value on every build, so Claude Code
# and hc are always fresh while the layers above stay cached.
ARG DOCKERCLAUDE_BUILD_ID=0

# Claude Code: ~/.local/bin/claude with versions in ~/.local/share/claude, so
# the bind mount over ~/.claude does not shadow the binary.
# hc (herdr-core): ~/.local/bin/herdr-core with ~/.local/bin/hc beside it.
RUN echo "build ${DOCKERCLAUDE_BUILD_ID}" \
    && curl -fsSL https://claude.ai/install.sh | bash \
    && curl -fsSL https://releases.herdr.dev/core/install.sh | sh \
    && claude --version \
    && hc --version

ENTRYPOINT ["/entrypoint.sh"]
