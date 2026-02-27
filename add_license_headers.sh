#!/usr/bin/env bash
# Adds a proprietary license header to all Rust source files under src/

YEAR=$(date +%Y)

HEADER="// Copyright (c) ${YEAR} Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.
"

find src -name '*.rs' | while read -r file; do
  if head -1 "$file" | grep -q "^// Copyright (c)" ; then
    echo "Skipping (already has header): $file"
    continue
  fi

  tmp=$(mktemp)
  printf '%s\n' "$HEADER" | cat - "$file" > "$tmp" && mv "$tmp" "$file"
  echo "Added header: $file"
done
