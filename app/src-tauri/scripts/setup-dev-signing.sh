#!/bin/sh
# One-time: create a self-signed code-signing certificate called "Spinamp Dev" in your login
# keychain, so dev builds keep a stable signature (see sign-and-run.sh). macOS will ask for your
# password to trust the certificate. Nothing leaves your machine.
set -e
if security find-identity -v -p codesigning | grep -q "Spinamp Dev"; then
  echo "Already set up."; exit 0
fi
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
cat > "$T/openssl.cnf" <<'CNF'
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = Spinamp Dev
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
CNF
openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -config "$T/openssl.cnf" -keyout "$T/key.pem" -out "$T/cert.pem" 2>/dev/null
openssl pkcs12 -export -legacy -inkey "$T/key.pem" -in "$T/cert.pem" -out "$T/id.p12" -passout pass:spinamp 2>/dev/null \
  || openssl pkcs12 -export -inkey "$T/key.pem" -in "$T/cert.pem" -out "$T/id.p12" -passout pass:spinamp
KC="$HOME/Library/Keychains/login.keychain-db"
security import "$T/id.p12" -k "$KC" -P spinamp -T /usr/bin/codesign
security add-trusted-cert -r trustRoot -p codeSign -k "$KC" "$T/cert.pem"
echo "Done. Check with: security find-identity -v -p codesigning   (you should see Spinamp Dev)"
