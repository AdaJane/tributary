# Assembled by the release workflow: the per-arch tribd binaries (built
# natively by the matrix legs, consoles embedded) are staged as
# <context>/{amd64,arm64}/tribd before `docker buildx build`, so the only
# emulated work in a multi-arch build is this apt-get.
FROM debian:trixie-slim
ARG TARGETARCH

# t64 name on trixie; pulseaudio-utils gives pactl/parec for the
# friendly-named patchbay when a Pulse/PipeWire socket is mounted in —
# without one the daemon falls back to raw ALSA (--device /dev/snd).
RUN apt-get update \
 && apt-get install -y --no-install-recommends libasound2t64 pulseaudio-utils \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --create-home --home-dir /var/lib/tributary tributary \
 && mkdir /projects && chown tributary /projects

# upload-artifact drops file modes; --chmod restores exec (handoff contract).
COPY --chmod=755 ${TARGETARCH}/tribd /usr/local/bin/tribd

# The bundled SoundFonts, staged beside the binaries by the release
# workflow. Read-only and shared by every container from this image, so
# they sit in /usr/share rather than under the service user's home.
COPY --chmod=644 soundfonts/ /usr/share/tributary/soundfonts/

# Loopback inside a container is unreachable; bind wide and publish with -p.
# Browser access via localhost passes the loopback origin check; LAN
# hostname/IP access passes the same-origin `.local`/IP-literal rule.
ENV TRIB__SERVER__BIND=0.0.0.0:4600 \
    TRIB__PROJECTS__ROOT=/projects

# The default config path is relative — from here, the daemon-written
# recording.toml prefs land in a directory the service user owns.
WORKDIR /var/lib/tributary
USER tributary
VOLUME /projects
EXPOSE 4600
# tribd traps SIGINT; make `docker stop` graceful.
STOPSIGNAL SIGINT
CMD ["tribd"]
