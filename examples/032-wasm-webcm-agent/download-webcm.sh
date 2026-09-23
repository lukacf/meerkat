#!/usr/bin/env bash
# Sourceable cache helper. Only a complete, nonempty pair is reusable.
download_webcm() {
  local base="$1" directory="$2"
  if [[ -s "$directory/webcm.mjs" && -s "$directory/webcm.wasm" ]]; then
    echo "WebCM already downloaded"
    return
  fi
  mkdir -p "$directory"
  local js="$directory/webcm.mjs.part" wasm="$directory/webcm.wasm.part"
  if ! curl -fSL "$base/webcm.mjs" -o "$js" ||
     ! curl -fSL "$base/webcm.wasm" -o "$wasm" ||
     [[ ! -s "$js" || ! -s "$wasm" ]]; then
    rm -f "$js" "$wasm"
    return 1
  fi
  mv "$js" "$directory/webcm.mjs"
  mv "$wasm" "$directory/webcm.wasm"
}
