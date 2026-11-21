FROM docker.io/library/alpine:3.24.4@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf

COPY build-inputs.sh /usr/local/lib/marsa/build-inputs.sh
COPY rootfs/ /usr/local/lib/marsa/rootfs/
USER 65532:65532
ENTRYPOINT ["/bin/sh", "/usr/local/lib/marsa/build-inputs.sh"]
