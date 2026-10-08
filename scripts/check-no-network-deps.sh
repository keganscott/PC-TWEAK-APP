#!/usr/bin/env bash
# The default (Starter) build must not depend on any networking crate
# (plan section 6.4: "collects nothing"; Store copy may say so only while this
# holds). Fails on any crate from the deny list, or on tokio's `net` feature.
#
# The app is not network-free: the connection check (catalogue E4, DECISIONS
# 15.22) sends ICMP echo requests, what `ping` sends, when the user starts it.
# It uses Windows' IP Helper, not a networking crate, so this check still
# holds; `network_audit.rs` keeps that the only network code in the source.
#
# Anything that needs the network later (licence server, AI Rig Engineer) must
# sit behind a non-default Cargo feature so this check keeps passing on the
# default build.
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET="${1:-x86_64-pc-windows-msvc}"
DENY='^(reqwest|hyper|hyper-util|hyper-rustls|hyper-tls|h2|h3|quinn|mio|socket2|rustls|rustls-pemfile|rustls-native-certs|native-tls|openssl|openssl-sys|ureq|curl|curl-sys|isahc|surf|attohttpc|tungstenite|tokio-tungstenite|tokio-rustls|tokio-native-tls|webpki|webpki-roots|tower-http|hickory-resolver|trust-dns-resolver|async-std|smol|isahc)$'

deps=$(cargo tree -p peaktweaks --target "$TARGET" -e normal --prefix none --no-dedupe 2>/dev/null | sed 's/ v[0-9].*//' | sort -u)
count=$(printf '%s\n' "$deps" | wc -l)
hits=$(printf '%s\n' "$deps" | grep -E "$DENY" || true)
if [ -n "$hits" ]; then
  echo "FAIL: networking crates in the default build:" >&2
  printf '  %s\n' $hits >&2
  echo "Show why with: cargo tree -p peaktweaks --target $TARGET -i <crate>" >&2
  exit 1
fi

if cargo tree -p peaktweaks --target "$TARGET" -e features -i tokio 2>/dev/null | grep -q 'tokio feature "net"'; then
  echo "FAIL: tokio's \`net\` feature is enabled" >&2
  exit 1
fi

echo "OK: none of the denied networking crates is among the $count crates in the default build for $TARGET"
