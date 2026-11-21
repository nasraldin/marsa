FROM docker.io/library/alpine:3.24.4@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf AS inputs

COPY build-inputs.sh /usr/local/lib/marsa/build-inputs.sh
COPY rootfs/ /usr/local/lib/marsa/rootfs/
RUN set -eu; \
    recipe=/usr/local/lib/marsa/rootfs; \
    branch=$(cut -d . -f 1,2 /etc/alpine-release); \
    architecture=$(apk --print-arch); \
    apks=$(mktemp -d); \
    /bin/sh "$recipe/fetch-packages.sh" \
        "https://dl-cdn.alpinelinux.org/alpine/v$branch/main/$architecture" \
        "$recipe/builder/$architecture.sha256" "$apks"; \
    apk --cache=no --network=no --repositories-file /dev/null --logfile=no \
        --scripts=no --force-non-repository add "$apks"/*.apk; \
    rm -rf -- "$apks"
USER 65532:65532
ENTRYPOINT ["/bin/sh", "/usr/local/lib/marsa/build-inputs.sh"]

FROM inputs AS image
USER 0:0
COPY image/ /usr/local/lib/marsa/image/
RUN set -eu; \
    recipe=/usr/local/lib/marsa; \
    branch=$(cut -d . -f 1,2 /etc/alpine-release); \
    architecture=$(apk --print-arch); \
    apks=$(mktemp -d); \
    for repository in main community; do \
        /bin/sh "$recipe/rootfs/fetch-packages.sh" \
            "https://dl-cdn.alpinelinux.org/alpine/v$branch/$repository/$architecture" \
            "$recipe/image/builder/$repository/$architecture.sha256" "$apks"; \
    done; \
    apk --cache=no --network=no --repositories-file /dev/null --logfile=no \
        --scripts=no --force-non-repository add "$apks"/*.apk; \
    rm -rf -- "$apks"
USER 65532:65532
ENTRYPOINT ["/bin/sh", "/usr/local/lib/marsa/image/build.sh"]
