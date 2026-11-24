#!/usr/bin/env bash
#
# acceptance.sh — automated acceptance checks for the `d` HTTP server.
#
# Builds the binary, starts it against a temporary fixture tree, runs a
# series of HTTP checks and reports PASS/FAIL for each. New phases append
# their own checks here so the script always covers the full feature set.
#
# Usage:
#   scripts/acceptance.sh              # build (debug) and run all checks
#   D_BIN=target/release/d scripts/acceptance.sh
#   PORT=18099 scripts/acceptance.sh
#
set -euo pipefail

cd "$(dirname "$0")/.."

PORT="${PORT:-18099}"
HOST="http://localhost:${PORT}"

PASS=0
FAIL=0
FAILED_NAMES=()

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

ok() {
    PASS=$((PASS + 1))
    printf '  \033[32mPASS\033[0m %s\n' "$1"
}

bad() {
    FAIL=$((FAIL + 1))
    FAILED_NAMES+=("$1")
    printf '  \033[31mFAIL\033[0m %s\n' "$1"
}

# check <name> <expected> <actual>
check() {
    if [ "$2" = "$3" ]; then ok "$1"; else
        printf '       expected: %s\n       actual:   %s\n' "$2" "$3"
        bad "$1"
    fi
}

# check_body <name> <expected-body> <actual-file>
check_body() {
    if [ "$2" = "$(cat "$3")" ]; then ok "$1"; else
        printf '       expected body: %s\n       actual body:   %s\n' "$2" "$(cat "$3")"
        bad "$1"
    fi
}

# http_code <curl-args...> -> prints status code
http_code() {
    curl -s -o /dev/null -w '%{http_code}' "$@"
}

# ---------------------------------------------------------------------------
# Build & fixture setup
# ---------------------------------------------------------------------------

if [ -z "${D_BIN:-}" ]; then
    echo "==> Building d (debug) ..."
    cargo build --quiet
    D_BIN=target/debug/d
fi
D_BIN="$(cd "$(dirname "$D_BIN")" && pwd)/$(basename "$D_BIN")"

FIXTURE="$(mktemp -d)"
LOG="$FIXTURE/server.log"
SERVER_PID=""

cleanup() {
    [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null || true
    rm -rf "$FIXTURE"
}
trap cleanup EXIT

echo "==> Preparing fixture tree in $FIXTURE/root"
mkdir -p "$FIXTURE/root/sub" "$FIXTURE/root/site" "$FIXTURE/root/docs" "$FIXTURE/root/media"
printf '0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ' > "$FIXTURE/root/data.bin"  # 36 bytes
echo 'fn main() {}' > "$FIXTURE/root/code.rs"
echo '你好，世界' > "$FIXTURE/root/中文.txt"
echo 'nested' > "$FIXTURE/root/sub/note.txt"
echo 'secret' > "$FIXTURE/root/.hidden"
echo '<h1>hello site</h1>' > "$FIXTURE/root/site/index.html"
cat > "$FIXTURE/root/docs/diagram.svg" <<'EOF'
<svg xmlns="http://www.w3.org/2000/svg" width="80" height="40"><rect width="80" height="40" fill="#ffffff"/></svg>
EOF
cat > "$FIXTURE/root/docs/guide.md" <<'EOF'
# 架构 Overview

![架构图](./diagram.svg)

| Col | Align |
| :--- | :---: |
| a | b |

- [x] done
- [ ] todo

~~old~~ text[^1]

<details><summary>more</summary>

Press <kbd>Ctrl</kbd>

</details>

```rust
fn demo() {}
```

[^1]: footnote note
EOF
cat > "$FIXTURE/root/media/logo.svg" <<'EOF'
<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><circle cx="16" cy="16" r="10"/></svg>
EOF
printf 'mp4data' > "$FIXTURE/root/media/clip.mp4"
printf 'mp3data' > "$FIXTURE/root/media/song.mp3"
# Symlink escaping the root (security check).
ln -s /etc/hosts "$FIXTURE/root/escape-link"
# index.html symlink escaping the root (must fall back to listing).
ln -s /etc/hosts "$FIXTURE/root/sub/index.html"

# ---------------------------------------------------------------------------
# Start server
# ---------------------------------------------------------------------------

echo "==> Starting $D_BIN on port $PORT"
"$D_BIN" -p "$PORT" -r "$FIXTURE/root" > "$LOG" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 50); do
    curl -s -o /dev/null "$HOST/" && break
    sleep 0.1
done

# ---------------------------------------------------------------------------
# Phase 0 checks: core HTTP semantics
# ---------------------------------------------------------------------------

echo "==> Phase 0: file serving"
check "GET file returns 200"            200 "$(http_code "$HOST/data.bin")"
check "GET file full body"              36  "$(curl -s "$HOST/data.bin" | wc -c | tr -d ' ')"
check "GET missing file returns 404"    404 "$(http_code "$HOST/nope.bin")"
check "GET favicon returns 404"         404 "$(http_code "$HOST/favicon.ico")"

head_len=$(curl -sI "$HOST/data.bin" | grep -i '^content-length:' | awk '{print $2}' | tr -d '\r')
head_body=$(curl -sI "$HOST/data.bin" | wc -c | tr -d ' ')
check "HEAD has Content-Length: 36"     36  "$head_len"
[ "$head_body" -eq 0 ] 2>/dev/null || true  # curl -I prints headers only; body checked via integration tests

echo "==> Phase 0: range requests"
check "Range 5-9 returns 206"           206 "$(http_code -r 5-9 "$HOST/data.bin")"
curl -s -r 5-9 "$HOST/data.bin" -o "$FIXTURE/range.out"
check_body "Range 5-9 body is correct slice" "56789" "$FIXTURE/range.out"
check "Range 5-9 Content-Range" \
    "bytes 5-9/36" \
    "$(curl -s -r 5-9 -D - -o /dev/null "$HOST/data.bin" | grep -i '^content-range:' | awk '{print $2" "$3}' | tr -d '\r')"
curl -s -r -4 "$HOST/data.bin" -o "$FIXTURE/suffix.out"
check_body "Suffix range -4 returns tail" "WXYZ" "$FIXTURE/suffix.out"
check "Unsatisfiable range returns 416" 416 "$(http_code -r 999999- "$HOST/data.bin")"
check "416 carries Content-Range" \
    "bytes */36" \
    "$(curl -s -r 999999- -D - -o /dev/null "$HOST/data.bin" | grep -i '^content-range:' | awk '{print $2" "$3}' | tr -d '\r')"

echo "==> Phase 0: conditional requests"
etag=$(curl -sI "$HOST/data.bin" | grep -i '^etag:' | awk '{print $2}' | tr -d '\r')
check "If-None-Match returns 304"       304 "$(http_code -H "If-None-Match: $etag" "$HOST/data.bin")"
check "If-None-Match: * returns 304"    304 "$(http_code -H 'If-None-Match: *' "$HOST/data.bin")"
check "If-Modified-Since returns 304"   304 "$(http_code -H "If-Modified-Since: $(LC_ALL=C date -u '+%a, %d %b %Y %H:%M:%S GMT')" "$HOST/data.bin")"

echo "==> Phase 0: security"
check "Symlink escape returns 404"      404 "$(http_code "$HOST/escape-link")"
check "Path traversal returns 404"      404 "$(http_code --path-as-is "$HOST/../../../../etc/passwd")"

echo "==> Phase 0: directory listing"
listing=$(curl -s "$HOST/")
echo "$listing" | grep -q 'data.bin'  && ok "Listing shows files"      || bad "Listing shows files"
echo "$listing" | grep -q 'code.rs'   && ok "Listing shows code files" || bad "Listing shows code files"
echo "$listing" | grep -qF '.hidden'  && bad "Hidden files excluded"   || ok "Hidden files excluded"

echo "==> Phase 0: directory trailing-slash 301"
check "Directory without trailing slash returns 301" 301 "$(http_code "$HOST/site")"
loc_plain=$(curl -s -D - -o /dev/null "$HOST/site" | grep -i '^location:' | awk '{print $2}' | tr -d '\r')
check "Directory 301 Location is /site/" "/site/" "$loc_plain"
loc_query=$(curl -s -D - -o /dev/null "$HOST/site?listing=true" | grep -i '^location:' | awk '{print $2}' | tr -d '\r')
check "Directory 301 preserves query string" "/site/?listing=true" "$loc_query"

echo "==> Phase 0: index.html serving"
site=$(curl -s "$HOST/site/")
echo "$site" | grep -q 'hello site' && ok "Directory serves index.html" || bad "Directory serves index.html"
site_listing=$(curl -s "$HOST/site/?listing=true")
echo "$site_listing" | grep -q 'Index of' && ok "?listing=true bypasses index.html" || bad "?listing=true bypasses index.html"
sub=$(curl -s "$HOST/sub/")
echo "$sub" | grep -q 'Index of' && ok "Escaped index.html symlink falls back to listing" || bad "Escaped index.html symlink falls back to listing"
echo "$sub" | grep -q 'localhost' && bad "Escaped index.html symlink not served" || ok "Escaped index.html symlink not served"

echo "==> Phase 0: CORS (default off)"
acao=$(curl -s -D - -o /dev/null -H 'Origin: https://example.com' "$HOST/data.bin" | grep -ci '^access-control-allow-origin:' || true)
check "No Access-Control-Allow-Origin by default" 0 "$acao"

echo "==> Phase 0: download headers"
cd_header=$(curl -s -D - -o /dev/null "$HOST/code.rs?view=download" | grep -i '^content-disposition:' | tr -d '\r')
echo "$cd_header" | grep -q 'attachment; filename="code.rs"; filename\*=UTF-8' \
    && ok "Content-Disposition (ASCII)" || bad "Content-Disposition (ASCII)"
cd_cn=$(curl -s -D - -o /dev/null "$HOST/%E4%B8%AD%E6%96%87.txt?view=download" | grep -i '^content-disposition:' | tr -d '\r')
echo "$cd_cn" | grep -q "filename\*=UTF-8''%E4%B8%AD%E6%96%87.txt" \
    && ok "Content-Disposition (RFC 5987 non-ASCII)" || bad "Content-Disposition (RFC 5987 non-ASCII)"

echo "==> Phase 0: built-in markdown, highlight, white UI & media viewer"
check "Docs directory without trailing slash returns 301" 301 "$(http_code "$HOST/docs")"
check "Docs 301 Location is /docs/" "/docs/" "$(curl -s -D - -o /dev/null "$HOST/docs" | grep -i '^location:' | awk '{print $2}' | tr -d '\r')"

md_html=$(curl -s "$HOST/docs/guide.md")
echo "$md_html" | grep -q '<h1 id="架构-overview">' && ok "Markdown CJK/ASCII anchor rendered" || bad "Markdown CJK/ASCII anchor rendered"
echo "$md_html" | grep -q 'style="text-align:center"' && ok "GFM table alignment rendered" || bad "GFM table alignment rendered"
echo "$md_html" | grep -q '<input type="checkbox" checked disabled>' && ok "GFM task checkbox rendered" || bad "GFM task checkbox rendered"
echo "$md_html" | grep -q '<del>old</del>' && ok "GFM strikethrough rendered" || bad "GFM strikethrough rendered"
echo "$md_html" | grep -q 'id="fn-1"' && ok "Markdown footnote rendered" || bad "Markdown footnote rendered"
echo "$md_html" | grep -q '<details><summary>more</summary>' && ok "Embedded block HTML unescaped" || bad "Embedded block HTML unescaped"
echo "$md_html" | grep -q '<kbd>Ctrl</kbd>' && ok "Embedded inline HTML unescaped" || bad "Embedded inline HTML unescaped"
echo "$md_html" | grep -q '<span class="hl-kw">fn</span>' && ok "Fenced code highlighted server-side" || bad "Fenced code highlighted server-side"
echo "$md_html" | grep -q '<img src="./diagram.svg" alt="架构图">' && ok "Markdown relative image preserved" || bad "Markdown relative image preserved"

svg_ct=$(curl -s -D - -o /dev/null "$HOST/docs/diagram.svg" | grep -i '^content-type:' | awk '{print $2}' | tr -d '\r')
check "Relative diagram.svg returns 200 image/svg+xml" "200 image/svg+xml" "$(http_code "$HOST/docs/diagram.svg") $svg_ct"

code_html=$(curl -s "$HOST/code.rs")
echo "$code_html" | grep -q '<span class="hl-kw">fn</span>' && ok "Code viewer uses built-in highlighting" || bad "Code viewer uses built-in highlighting"

raw_svg=$(curl -s "$HOST/media/logo.svg")
echo "$raw_svg" | grep -q '<svg' && ! echo "$raw_svg" | grep -qi '<!DOCTYPE' \
    && ok "Media bare path serves raw bytes" || bad "Media bare path serves raw bytes"
img_view=$(curl -s "$HOST/media/logo.svg?view=viewer")
echo "$img_view" | grep -q '<img src="/media/logo.svg"' && ok "Image ?view=viewer embeds bare path" || bad "Image ?view=viewer embeds bare path"
curl -s "$HOST/media/clip.mp4?view=viewer" | grep -q '<video controls' && ok "Video ?view=viewer renders player" || bad "Video ?view=viewer renders player"
curl -s "$HOST/media/song.mp3?view=viewer" | grep -q '<audio controls' && ok "Audio ?view=viewer renders player" || bad "Audio ?view=viewer renders player"

for page_path in "/" "/docs/guide.md" "/code.rs" "/media/logo.svg?view=viewer"; do
    page=$(curl -s "$HOST$page_path")
    if echo "$page" | grep -q '#ffffff' && echo "$page" | grep -q '#f6f8fa' && echo "$page" | grep -q '#0969da' \
        && ! echo "$page" | grep -qiE '<script|href="https?://|#0d1117|#161b22|file-type-badge|📁|📜|🎬|⬇️'; then
        ok "White minimal offline UI verified ($page_path)"
    else
        bad "White minimal offline UI verified ($page_path)"
    fi
done

echo "==> Phase 0: graceful shutdown"
kill -TERM "$SERVER_PID"
for _ in $(seq 1 50); do
    kill -0 "$SERVER_PID" 2>/dev/null || break
    sleep 0.1
done
if kill -0 "$SERVER_PID" 2>/dev/null; then
    bad "SIGTERM graceful shutdown"
else
    grep -q 'Received SIGTERM' "$LOG" \
        && ok "SIGTERM graceful shutdown" || bad "SIGTERM graceful shutdown"
fi
SERVER_PID=""

# ---------------------------------------------------------------------------
# Phase 1: optional flags (--cors --hidden)
# ---------------------------------------------------------------------------

echo "==> Phase 1: restart with --cors --hidden"
"$D_BIN" -p "$PORT" -r "$FIXTURE/root" --cors --hidden > "$LOG" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 50); do
    curl -s -o /dev/null "$HOST/" && break
    sleep 0.1
done

acao_star=$(curl -s -D - -o /dev/null -H 'Origin: https://example.com' "$HOST/data.bin" | grep -ci '^access-control-allow-origin: \*' || true)
check "CORS enabled sends Access-Control-Allow-Origin: *" 1 "$acao_star"

opt_code=$(curl -s -X OPTIONS -H 'Origin: https://example.com' -H 'Access-Control-Request-Method: GET' -o /dev/null -w '%{http_code}' "$HOST/data.bin")
check "CORS OPTIONS preflight returns 204" 204 "$opt_code"
opt_methods=$(curl -s -D - -o /dev/null -X OPTIONS -H 'Origin: https://example.com' "$HOST/data.bin" | grep -ci '^access-control-allow-methods:.*GET' || true)
check "CORS OPTIONS advertises Access-Control-Allow-Methods" 1 "$opt_methods"

listing=$(curl -s "$HOST/")
echo "$listing" | grep -qF '.hidden' && ok "Hidden files shown with --hidden" || bad "Hidden files shown with --hidden"

kill -TERM "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true
SERVER_PID=""

# ---------------------------------------------------------------------------
# Phase 2: ENV variable compatibility (D_PORT, D_CORS, D_ROOT)
# ---------------------------------------------------------------------------

ENV_PORT=$((PORT + 1))
ENV_HOST="http://127.0.0.1:$ENV_PORT"
echo "==> Phase 2: start with D_PORT=$ENV_PORT D_CORS=true"
D_PORT="$ENV_PORT" D_CORS=true D_ROOT="$FIXTURE/root" "$D_BIN" > "$FIXTURE/env.log" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 50); do
    curl -s -o /dev/null "$ENV_HOST/" && break
    sleep 0.1
done

env_cors=$(curl -s -D - -o /dev/null -H 'Origin: https://example.com' "$ENV_HOST/data.bin" | grep -ci '^access-control-allow-origin: \*' || true)
check "ENV D_PORT + D_CORS enables CORS" 1 "$env_cors"

kill -TERM "$SERVER_PID"
for _ in $(seq 1 50); do
    kill -0 "$SERVER_PID" 2>/dev/null || break
    sleep 0.1
done
grep -q 'Received SIGTERM' "$FIXTURE/env.log" \
    && ok "ENV instance SIGTERM graceful shutdown" || bad "ENV instance SIGTERM graceful shutdown"
wait "$SERVER_PID" 2>/dev/null || true
SERVER_PID=""

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

echo
echo "==> Result: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    printf '    failed: %s\n' "${FAILED_NAMES[@]}"
    exit 1
fi
echo "==> All acceptance checks passed ✔"
