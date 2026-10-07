#!/usr/bin/env bash
BIN="/x/ndkbin"
build() {
  triple="$1"
  cc_name="$2"
  upper="$(echo "$triple" | tr 'a-z-' 'A-Z_')"
  export "CC_${upper}"="$BIN/$cc_name"
  echo "VARNAME=CC_${upper}"
  eval "val=\${CC_${upper}}"
  echo "VALUE=$val"
}
build aarch64-linux-android aarch64-linux-android24-clang
