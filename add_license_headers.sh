#!/usr/bin/env bash
# Adds a proprietary license header to all Rust source files in the provided directories.

# Check if at least one argument (directory) was provided
if [ "$#" -eq 0 ]; then
  echo "Usage: $0 <dir1> [dir2] [dir3] ..."
  exit 1
fi

YEAR=$(date +%Y)

HEADER="// Copyright (c) ${YEAR} Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.
"

# Pass all script arguments ("$@") to the find command
find "$@" -type f -name '*.rs' | while read -r file; do
  if head -1 "$file" | grep -q "^// Copyright (c)" ; then
    echo "Skipping (already has header): $file"
    continue
  fi

  tmp=$(mktemp)
  printf '%s\n' "$HEADER" | cat - "$file" > "$tmp" && mv "$tmp" "$file"
  echo "Added header: $file"
done