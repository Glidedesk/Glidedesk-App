#!/bin/sh
# Creates Glidedesk's signing keys ONCE — no paid account needed.
#   macOS  : self-signed code-signing identity (stable → permissions survive updates)
#   Windows: self-signed Authenticode certificate
#   Linux  : minisign key for SHA256SUMS
# Private material goes to .signing/ (git-ignored). Public parts go to signing/.
# Run inside the builder:  make signing-keys
set -eu
cd "$(dirname "$0")/.."
out=.signing
if [ -e "$out/password" ]; then
  echo "Keys already exist in $out/ — delete it first to create new ones." >&2
  exit 1
fi
mkdir -p "$out" signing
chmod 700 "$out"
umask 077
openssl rand -base64 24 | tr -d '\n' > "$out/password"
pw="$(cat "$out/password")"

cert() { # name cn
  openssl req -x509 -newkey rsa:3072 -sha256 -days 3650 -nodes \
    -keyout "$out/$1.key" -out "$out/$1.crt" -subj "/CN=$2/O=Glidedesk" \
    -addext "basicConstraints=critical,CA:FALSE" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=critical,codeSigning" 2>/dev/null
  # Classic PKCS#12 encryption: readable by rcodesign, osslsigncode and Windows.
  openssl pkcs12 -export -inkey "$out/$1.key" -in "$out/$1.crt" -name "$2" \
    -certpbe PBE-SHA1-3DES -keypbe PBE-SHA1-3DES -macalg sha1 \
    -out "$out/$1.p12" -passout "pass:$pw"
}
cert mac "Glidedesk Code Signing"
cert win "Glidedesk"
openssl x509 -in "$out/win.crt" -outform DER -out signing/glidedesk-codesign.cer
cp "$out/mac.crt" signing/glidedesk-macos-codesign.pem
chmod 644 signing/glidedesk-codesign.cer signing/glidedesk-macos-codesign.pem
minisign -G -W -p signing/minisign.pub -s "$out/minisign.key" >/dev/null
chmod 644 signing/minisign.pub

# Values for GitHub → Settings → Secrets and variables → Actions.
{
  echo "GLIDEDESK_SIGN_PASSWORD=$pw"
  echo "GLIDEDESK_MAC_P12_BASE64=$(base64 -w0 "$out/mac.p12")"
  echo "GLIDEDESK_WIN_PFX_BASE64=$(base64 -w0 "$out/win.p12")"
  echo "GLIDEDESK_MINISIGN_KEY_BASE64=$(base64 -w0 "$out/minisign.key")"
} > "$out/github-secrets.env"
chmod 600 "$out"/*
cat <<MSG
Signing keys created.
  Private (keep safe, never commit): $out/
  Public (commit these):             signing/glidedesk-codesign.cer, signing/glidedesk-macos-codesign.pem, signing/minisign.pub
For GitHub Actions add the 4 secrets in $out/github-secrets.env, e.g. with the GitHub CLI:
  while IFS='=' read -r k v; do gh secret set "\$k" --body "\$v"; done < $out/github-secrets.env
Local builds (make release) use $out/ automatically.
MSG
