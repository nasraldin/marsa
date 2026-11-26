FROM alpine:3.24.4@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf
ARG PROOF
RUN printf '%s %s\n' "$(apk --print-arch)" "$PROOF" >/build-proof
ENTRYPOINT ["/bin/sh", "-ec", "cat /build-proof && apk --print-arch"]
