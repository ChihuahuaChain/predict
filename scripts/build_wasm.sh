#!/usr/bin/env bash
# Reproducible optimized build of the prediction contract into artifacts/.
set -euo pipefail
cd "$(dirname "$0")/.."

docker run --rm -v "$(pwd)":/code \
  --mount type=volume,source=huahua_predict_cache,target=/target \
  --mount type=volume,source=registry_cache,target=/usr/local/cargo/registry \
  cosmwasm/optimizer:0.17.0

# Mainnet (wasmvm 2.2) capabilities; keep in sync with chihuahua app/app.go.
CAPS="iterator,stargate,staking,cosmwasm_1_1,cosmwasm_1_2,cosmwasm_1_3,cosmwasm_1_4,cosmwasm_2_0"
if command -v cosmwasm-check >/dev/null; then
  cosmwasm-check --available-capabilities "$CAPS" artifacts/huahua_prediction.wasm
else
  echo "cosmwasm-check not installed: cargo install cosmwasm-check --version 2.2.2" >&2
fi
ls -la artifacts/
