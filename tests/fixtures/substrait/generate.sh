#!/usr/bin/env bash
set -euo pipefail

# Pass the proto directory from the substrait 0.63.0 crate (wire 0.85.0).
check=false
if [[ ${1:-} == --check ]]; then
    check=true
    shift
fi
if [[ $# != 1 ]]; then
    printf '%s\n' 'usage: bash generate.sh [--check] /path/to/substrait/proto' >&2
    exit 2
fi
proto_dir=$(cd "$1" && pwd)
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"

scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
for fixture in sine_gain modulation fan_out; do
    encoded="$scratch/$fixture.pb"
    protoc -I "$proto_dir" -I . --encode=substrait.Plan \
        substrait/plan.proto sonicfusion_spike.proto \
        < "$fixture.textproto" > "$encoded"
    if $check; then
        if ! cmp -s "$encoded" "$fixture.pb"; then
            printf '%s differs from %s.textproto; regenerate it\n' "$fixture.pb" "$fixture" >&2
            exit 1
        fi
    else
        mv "$encoded" "$fixture.pb"
    fi
done
