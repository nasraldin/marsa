#!/bin/sh
# Generate a disposable TLS identity and overlay a non-shipping guest fixture.
set -eu
umask 077
base=$1
fixture=$2
install -d -m 0700 "$fixture"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 \
  -subj '/CN=Marsa Disposable Proxy Test CA' \
  -addext 'basicConstraints=critical,CA:TRUE' \
  -addext 'keyUsage=critical,keyCertSign,cRLSign' \
  -out "$fixture/ca.pem" -keyout "$fixture/ca.key" 2>"$fixture/openssl.log"
openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -subj '/CN=registry.marsa-proxy.invalid' \
  -out "$fixture/leaf.csr" -keyout "$fixture/leaf.key" 2>>"$fixture/openssl.log"
cat > "$fixture/leaf.ext" <<'EXTENSIONS'
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=serverAuth
subjectAltName=DNS:registry.marsa-proxy.invalid
EXTENSIONS
openssl x509 -req -days 1 -in "$fixture/leaf.csr" \
  -CA "$fixture/ca.pem" -CAkey "$fixture/ca.key" -CAcreateserial \
  -extfile "$fixture/leaf.ext" -out "$fixture/leaf.pem" 2>>"$fixture/openssl.log"
openssl verify -CAfile "$fixture/ca.pem" -verify_hostname registry.marsa-proxy.invalid "$fixture/leaf.pem"
helper=$3
shift 3
python3 "$(dirname "$0")/stage_guest.py" "$base" "$fixture" "$helper" "$@"
