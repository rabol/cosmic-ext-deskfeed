#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
#
# Milestone 1 research (ADR 0004): does Microsoft's public Remote Desktop
# client ID accept a loopback redirect? If yes, sign-in can use the system
# browser and no embedded web view is needed (ADR 0003).
#
# Unauthenticated checks are inconclusive: for /organizations, Microsoft
# validates the redirect only after the account is chosen (2026-10-17).
#
# This script only prints URLs. Nothing listens on the port, and the PKCE
# challenge is random with the verifier thrown away, so any code the browser
# receives cannot be redeemed. Codes also expire within minutes.

set -euo pipefail

client_id="a85cf173-4192-42f8-81fa-777a763e6e2c"
scope="https%3A%2F%2Fwww.wvd.microsoft.com%2F.default%20openid%20profile%20offline_access"
challenge="$(openssl rand 32 | openssl dgst -sha256 -binary | openssl base64 -A | tr '+/' '-_' | tr -d '=')"
state="$(openssl rand -hex 16)"
port=51234

url() {
    printf 'https://login.microsoftonline.com/organizations/oauth2/v2.0/authorize?client_id=%s&response_type=code&scope=%s&redirect_uri=%s&code_challenge=%s&code_challenge_method=S256&state=%s&prompt=select_account\n' \
        "$client_id" "$scope" "$1" "$challenge" "$state"
}

cat <<EOF
Open each URL in your normal browser and sign in with your work account.
For each, note which result you get:

  REJECTED: Microsoft shows an error page with AADSTS50011
            ("redirect URI ... does not match").
  ACCEPTED: the browser tries to open localhost/127.0.0.1 and shows
            "unable to connect", with ?code=... in the address bar.
  OTHER:    any other AADSTS error (note the number only).

Report only REJECTED / ACCEPTED / OTHER plus the AADSTS number.
Do not paste the address bar contents.

1) http://localhost:$port
$(url "http%3A%2F%2Flocalhost%3A$port")

2) http://127.0.0.1:$port
$(url "http%3A%2F%2F127.0.0.1%3A$port")

3) http://localhost (no port)
$(url "http%3A%2F%2Flocalhost")
EOF
