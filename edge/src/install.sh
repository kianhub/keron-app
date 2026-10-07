#!/bin/sh
# Keron relay: /install.sh
#
# Zeron served a Linux curl|sh installer here. Keron has no hosted builds, so
# this only says how to install from source. It never downloads anything.
set -eu

cat >&2 <<'EOF'
Keron isn't installed with curl. Build it from the keron-app repository:

  macOS app:   scripts/package-macos.sh   (fill in keron.toml first)
  CLI/daemon:  cargo build --release -p zeron   (binary: target/release/keron)

Then sign in and run the agent host as a background service:

  keron login
  keron daemon install
EOF
exit 1
