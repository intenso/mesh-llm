#!/usr/bin/env bash
# detect-llama-device.sh — pick the best llama.cpp device string for this host

set -euo pipefail

if [[ "$(uname -s)" == "Darwin" ]]; then
    echo MTL0
    exit 0
fi

if command -v nvidia-smi &>/dev/null; then
    if nvidia-smi --query-gpu=name --format=csv,noheader 2>/dev/null | grep -q '[^[:space:]]'; then
        echo CUDA0
        exit 0
    fi
fi

if command -v tegrastats &>/dev/null; then
    echo CUDA0
    exit 0
fi

if command -v rocm-smi &>/dev/null; then
    if rocm-smi --showproductname 2>/dev/null | grep -q '^GPU\['; then
        echo HIP0
        exit 0
    fi
fi

if command -v rocminfo &>/dev/null; then
    if rocminfo 2>/dev/null | grep -q 'gfx'; then
        echo HIP0
        exit 0
    fi
fi

if command -v vulkaninfo &>/dev/null; then
    if vulkaninfo --summary >/dev/null 2>&1; then
        echo Vulkan0
        exit 0
    fi
fi

echo CPU
