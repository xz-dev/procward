#!/usr/bin/env bash
# SPDX-License-Identifier: BSD-3-Clause
set -euo pipefail
exec docker run --rm -v "$PWD:$PWD" -w "$PWD" \
  "${PROCWARD_CI_IMAGE:?missing pinned target image}" "$@"
