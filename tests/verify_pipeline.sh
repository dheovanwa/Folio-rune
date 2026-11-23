#!/usr/bin/env bash
set -eo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKTREE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

cd "${WORKTREE_DIR}"

echo "=========================================================="
echo "Folio QA Verification & Headless Snapshot Pipeline"
echo "Worktree: ${WORKTREE_DIR}"
echo "=========================================================="

echo "[1/4] Running cargo check..."
cargo check

echo "[2/4] Running cargo clippy..."
CLIPPY_OUTPUT=$(cargo clippy 2>&1 || true)
CLIPPY_WARNINGS=$(echo "${CLIPPY_OUTPUT}" | grep -c "warning:" || true)
echo "Clippy warning count: ${CLIPPY_WARNINGS}"

echo "[3/4] Ensuring target/debug/folio binary is built..."
cargo build --bin folio

echo "[4/4] Generating headless Rune snapshots & JSON element trees..."
mkdir -p snaps
SCENES=(
    blank reading toc settings library light sepia dark charcoal
    chapter address find chapter-toc oled empty-lib
)

for scene in "${SCENES[@]}"; do
    ./target/debug/folio --scene "${scene}" --snap "snaps/${scene}.png" --tree "snaps/${scene}.json"
done

echo "[5/5] Running visual verifier and integration test suites..."
cargo test --test visual_verifier --test cache_and_library_tests

echo "=========================================================="
echo "All QA verification tests and headless visual checks PASSED!"
echo "Total scenes verified: ${#SCENES[@]}"
echo "=========================================================="
