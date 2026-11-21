FROM docker.io/library/alpine:3.24.4@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf

RUN apk add --no-cache \
    apparmor=4.1.7-r1 \
    binutils=2.45.1-r1 \
    gcc=15.2.0-r5 \
    make=4.4.1-r4 \
    musl-dev=1.2.6-r3 \
    bison=3.8.2-r3 \
    flex=2.6.4-r8 \
    bc=1.08.2-r1 \
    openssl-dev=3.5.9-r1 \
    perl=5.42.3-r0

COPY build.sh config /usr/local/lib/marsa/
USER 65532:65532
ENTRYPOINT ["/bin/sh", "/usr/local/lib/marsa/build.sh"]
