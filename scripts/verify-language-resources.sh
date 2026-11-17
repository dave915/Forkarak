#!/bin/bash
# Runtime/install-side verification using macOS system tools only. No Python/JDK execution.
set -euo pipefail
fail() { echo "Language resources: $*" >&2; exit 1; }
[[ $# -eq 1 && -d "$1" && ! -L "$1" ]] || fail 'Expected a language-tools directory'
ROOT="$(cd -P -- "$1" && pwd -P)"
[[ -z "$(/usr/bin/find "$ROOT" ! -type d ! -type f -print -quit)" ]] || fail 'Links/special files are forbidden'
MANIFEST="$ROOT/manifest.json"
SUMS="$ROOT/SHA256SUMS"
[[ -f "$MANIFEST" && -f "$SUMS" ]] || fail 'Missing manifest or integrity inventory'
get() { /usr/bin/plutil -extract "$1" raw -o - "$MANIFEST" 2>/dev/null; }
[[ "$(get schemaVersion)" == 1 ]] || fail 'Unsupported manifest version'
[[ "$(get packages.0.package)" == java && "$(get packages.0.distribution)" == Bundled ]] || fail 'Expected reviewed bundled Java support'
if get packages.1.package >/dev/null; then fail 'Unreviewed extra package'; fi
safe_relative() {
    [[ -n "$1" && "$1" != /* && "$1" != *$'\n'* && "$1" != *$'\r'* && "$1" != *'\'* ]] || return 1
    case "/$1/" in *'/../'*|*'/./'*|*'//'*) return 1 ;; esac
}
# Validate every inventory path before shasum is allowed to open one.
count=0
while IFS= read -r line || [[ -n "$line" ]]; do
    hash="${line:0:64}"
    path="${line:66}"
    [[ "$hash" =~ ^[0-9a-f]{64}$ && "${line:64:2}" == '  ' ]] || fail 'Invalid checksum entry'
    safe_relative "$path" || fail 'Unsafe checksum path'
    [[ "$path" != SHA256SUMS && -f "$ROOT/$path" ]] || fail "Missing checksum resource: $path"
    count=$((count + 1))
done < "$SUMS"
[[ "$count" -gt 0 ]] || fail 'Empty integrity inventory'
# Duplicate entries could otherwise hide an unlisted resource in a count check.
[[ "$(/usr/bin/cut -c 67- "$SUMS" | LC_ALL=C /usr/bin/sort -u | /usr/bin/wc -l | /usr/bin/tr -d ' ')" == "$count" ]] || fail 'Duplicate checksum entry'
actual="$(/usr/bin/find "$ROOT" -type f | /usr/bin/wc -l | /usr/bin/tr -d ' ')"
[[ "$actual" == "$((count + 1))" ]] || fail 'Unlisted language resource'
(cd "$ROOT" && /usr/bin/shasum -a 256 -c SHA256SUMS >/dev/null) || fail 'Resource checksum verification failed'
resource() {
    local path="$1"
    [[ "$path" == '{resources}/language-tools/'* ]] || fail 'Nonrelocatable resource path'
    path="${path#\{resources\}/language-tools/}"
    safe_relative "$path" || fail 'Unsafe manifest path'
    [[ -e "$ROOT/$path" ]] || fail "Missing manifest resource: $path"
    printf '%s' "$ROOT/$path"
}
for required in legal/NOTICE.md legal/SOURCE-ACCESS.md legal/provenance.json recipes/kotlin.json; do
    [[ -s "$ROOT/$required" ]] || fail "Missing legal resource: $required"
done
[[ "$(get packages.0.servers.0.language)" == java ]] || fail 'Missing Java command'
index=0
while program="$(get packages.0.servers.$index.program)"; do
    if [[ "$program" != /usr/bin/env ]]; then
        payload="$(resource "$program")"
        [[ -f "$payload" && -x "$payload" ]] || fail 'Java program is not executable'
    fi
    arg_index=0
    payload_seen=false
    while arg="$(get packages.0.servers.$index.args.$arg_index)"; do
        [[ "$arg" != *$'\n'* && "$arg" != *$'\r'* ]] || fail 'Invalid manifest argument'
        if [[ "$arg" == *=/* && "$arg" != 'PATH=/usr/bin:/bin:/usr/sbin:/sbin' ]]; then
            fail 'Absolute environment/option path outside Resources'
        fi
        if [[ "$program" == /usr/bin/env && "$payload_seen" == false && "$arg" != -* && "$arg" != *=* ]]; then
            payload="$(resource "$arg")"
            [[ -f "$payload" && -x "$payload" ]] || fail 'Env payload is not executable'
            payload_seen=true
        fi
        case "$arg" in
            *'{resources}'*)
                value="$arg"
                if [[ "$value" == *=* ]]; then value="${value#*=}"; fi
                resolved="$(resource "$value")"
                ;;
            /*) fail 'Absolute path outside Resources in arguments' ;;
        esac
        arg_index=$((arg_index + 1))
    done
    if [[ "$program" == /usr/bin/env ]]; then "$payload_seen" || fail 'Missing bundled env payload'; fi
    index=$((index + 1))
done
notice_index=0
while notice="$(get packages.0.notices.$notice_index)"; do
    resource "$notice" >/dev/null
    notice_index=$((notice_index + 1))
done
[[ "$notice_index" -gt 0 && -n "$(get packages.0.artifacts.0.checksum)" ]] || fail 'Missing legal provenance'
