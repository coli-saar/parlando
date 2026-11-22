#!/bin/sh
# Run the shared host as an unprivileged user, including on platforms assigning a UID.
set -eu
run_parlando() {
    if [ "$(id -u)" = 0 ]; then
        exec gosu parlando parlando "$@"
    fi
    exec parlando "$@"
}
if [ "$#" -gt 0 ]; then
    run_parlando "$@"
fi
# Only initialize this application's directory inside the provider-owned volume.
if [ "$(id -u)" = 0 ]; then
    install -d -m 0700 -o parlando -g parlando /data/parlando
else
    umask 077
    mkdir -p /data/parlando
fi
set -- serve --bind "0.0.0.0:${PORT:-8000}" --store /data/parlando
public_url=${PARLANDO_PUBLIC_URL:-${RENDER_EXTERNAL_URL:-}}
if [ -z "$public_url" ] && [ -n "${RAILWAY_PUBLIC_DOMAIN:-}" ]; then
    public_url="https://$RAILWAY_PUBLIC_DOMAIN"
fi
: "${public_url:?Set PARLANDO_PUBLIC_URL to the public HTTPS origin of this host}"
set -- "$@" --public-url "$public_url"
run_parlando "$@"
