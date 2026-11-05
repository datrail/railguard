#!/bin/sh
# Writes a throwaway test CA, and the TLS upstream's certificate and keystore, into the directory
# given, at every start: the keys are made for this run and never committed.
#
#   ca-certificates.crt   the CA, named as the system bundle Envoy trusts
#   upstream-tls.p12      the upstream's key and certificate, for WireMock (password: password)
set -eu

out=$1
# A run's files replace the last one's before anything reads them.
rm -f "$out"/*
work=$(mktemp -d)
cd "$work"

openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=railguard e2e test CA" \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign" \
  -keyout ca.key -out ca.crt 2>/dev/null

# One server for both names: the entry with a port, and the one reached at the agent's.
openssl req -newkey rsa:2048 -nodes -subj "/CN=upstream-tls.test" \
  -keyout upstream-tls.key -out upstream-tls.csr 2>/dev/null
printf 'subjectAltName=DNS:upstream-tls.test,DNS:upstream-agent-port.test\nextendedKeyUsage=serverAuth\n' \
  > upstream-tls.ext
openssl x509 -req -in upstream-tls.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days 2 \
  -extfile upstream-tls.ext -out upstream-tls.crt 2>/dev/null
openssl pkcs12 -export -in upstream-tls.crt -inkey upstream-tls.key -name upstream-tls.test \
  -out upstream-tls.p12 -passout pass:password

cp upstream-tls.p12 "$out/"
cp ca.crt "$out/ca-certificates.crt"
echo "certs: written to $out"
