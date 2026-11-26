#!/bin/sh
# Run exclusively in a throwaway account; discard that account after qualification.
set -eu
[ "$#" -eq 2 ] && [ "$(id -u)" != 0 ]
[ -n "${MARSA_TEST_DISPOSABLE_TRUST_USER:-}" ] &&
    [ "$(id -un)" = "$MARSA_TEST_DISPOSABLE_TRUST_USER" ] || {
        echo 'Select the dedicated disposable test account.' >&2
        exit 2
    }
case "$1" in
    trust) /usr/bin/security add-trusted-cert -r trustRoot -k "$HOME/Library/Keychains/login.keychain-db" "$2" ;;
    revoke) /usr/bin/security remove-trusted-cert "$2" ;;
    *) exit 2 ;;
esac
