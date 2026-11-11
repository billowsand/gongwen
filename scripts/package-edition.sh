#!/usr/bin/env bash
# Sourced by the native packaging scripts after setting EDITION.
case "$EDITION" in
    ai) EDITION_SUFFIX="" ;;
    no-ai) EDITION_SUFFIX="-no-ai" ;;
    *) echo "error: invalid edition: $EDITION (expected ai or no-ai)" >&2; exit 1 ;;
esac

check_package_binary() {
    local actual
    actual="$("$1" --build-edition)" || return 1
    if [ "$actual" != "$EDITION" ]; then
        echo "error: expected $EDITION binary, got '$actual': $1" >&2
        return 1
    fi
}

check_package_resources() {
    if [ "$EDITION" = no-ai ]; then
        local relative
        for relative in runtime/pi skills config.example.json; do
            if [ -e "$1/$relative" ] || [ -L "$1/$relative" ]; then
                echo "error: AI resource in no-ai staging: $1/$relative" >&2
                return 1
            fi
        done
    fi
}
