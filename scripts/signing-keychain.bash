#!/usr/bin/env bash
# On a CI runner, give scripts/dmg.bash a Developer ID identity and notary credentials,
# and name them to the steps that follow through GITHUB_ENV.
#
# MACOS_CERTIFICATE is the identity as a base64 PKCS #12 file, and
# MACOS_CERTIFICATE_PASSWORD opens it. NOTARY_APPLE_ID, NOTARY_TEAM_ID and
# NOTARY_PASSWORD, an app-specific password, sign in to Apple's notary service.
#
# ⚠️ Not for a personal machine: it adds a keychain to the user's search list and a
# notary profile to the login keychain.
# macOS only: security, openssl, base64 and xcrun are the system's.
set -euo pipefail

fail() {
  echo "$*" >&2
  exit 1
}

for name in MACOS_CERTIFICATE MACOS_CERTIFICATE_PASSWORD NOTARY_APPLE_ID NOTARY_TEAM_ID NOTARY_PASSWORD; do
  [[ -n ${!name:-} ]] || fail "$name is not set"
done

keychain="$RUNNER_TEMP/signing.keychain-db"
password="$(/usr/bin/openssl rand -hex 16)"
/usr/bin/security create-keychain -p "$password" "$keychain"
# A new keychain locks itself after a few minutes, and the build outlasts that.
/usr/bin/security set-keychain-settings -lut 21600 "$keychain"
/usr/bin/security unlock-keychain -p "$password" "$keychain"

/usr/bin/base64 --decode <<<"$MACOS_CERTIFICATE" >"$RUNNER_TEMP/identity.p12"
/usr/bin/security import "$RUNNER_TEMP/identity.p12" -f pkcs12 -P "$MACOS_CERTIFICATE_PASSWORD" \
  -T /usr/bin/codesign -k "$keychain"
rm "$RUNNER_TEMP/identity.p12"
# Without this, codesign stops at a dialog asking to use the key.
/usr/bin/security set-key-partition-list -S apple-tool:,apple: -k "$password" "$keychain" >/dev/null

searched=()
while IFS= read -r path; do
  searched+=("$path")
done < <(/usr/bin/security list-keychains -d user | sed -E 's/^ *"(.*)"$/\1/')
/usr/bin/security list-keychains -d user -s "$keychain" "${searched[@]}"

identity="$(/usr/bin/security find-identity -v -p codesigning "$keychain" |
  awk '/Developer ID Application/ { print $2; exit }')"
[[ -n $identity ]] || fail "MACOS_CERTIFICATE holds no valid Developer ID Application identity"

# This signs in to the notary service, so wrong credentials fail here.
/usr/bin/xcrun notarytool store-credentials drawbar-notary --apple-id "$NOTARY_APPLE_ID" \
  --team-id "$NOTARY_TEAM_ID" --password "$NOTARY_PASSWORD" >/dev/null

{
  echo "DRAWBAR_SIGN_IDENTITY=$identity"
  echo "DRAWBAR_NOTARY_PROFILE=drawbar-notary"
} >>"$GITHUB_ENV"
