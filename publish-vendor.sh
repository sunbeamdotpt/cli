#!/usr/bin/env bash
set -euo pipefail

VENDOR_DIR="$(cd "$(dirname "$0")/vendor" && pwd)"
REGISTRY="sunbeam"
MAX_ROUNDS=10

# Collect all crate directories
mapfile -t all_crates < <(find "$VENDOR_DIR" -maxdepth 1 -mindepth 1 -type d | sort)

echo "Found ${#all_crates[@]} vendored crates to publish."

# Remove Cargo.toml.orig files that cargo refuses to package
find "$VENDOR_DIR" -name "Cargo.toml.orig" -delete
echo "Cleaned up Cargo.toml.orig files."

remaining=("${all_crates[@]}")
round=1

while [[ ${#remaining[@]} -gt 0 && $round -le $MAX_ROUNDS ]]; do
    echo ""
    echo "=== Round $round: ${#remaining[@]} crates remaining ==="
    failed=()

    for crate_dir in "${remaining[@]}"; do
        crate_name=$(basename "$crate_dir")

        # Extract name and version from Cargo.toml
        name=$(grep -m1 '^name = ' "$crate_dir/Cargo.toml" | sed 's/name = "\(.*\)"/\1/')
        version=$(grep -m1 '^version = ' "$crate_dir/Cargo.toml" | sed 's/version = "\(.*\)"/\1/')

        echo -n "  Publishing $name@$version ... "

        if cargo publish \
            --registry "$REGISTRY" \
            --manifest-path "$crate_dir/Cargo.toml" \
            --no-verify \
            --allow-dirty \
            2>/tmp/cargo-publish-err.log; then
            echo "OK"
        else
            err=$(cat /tmp/cargo-publish-err.log)
            # If already published, treat as success
            if echo "$err" | grep -qi "already uploaded\|already exists"; then
                echo "SKIP (already published)"
            else
                echo "FAIL"
                failed+=("$crate_dir")
            fi
        fi
    done

    if [[ ${#failed[@]} -eq ${#remaining[@]} ]]; then
        echo ""
        echo "ERROR: No progress made in round $round. ${#failed[@]} crates stuck."
        echo "Last error:"
        cat /tmp/cargo-publish-err.log
        echo ""
        echo "Stuck crates:"
        for f in "${failed[@]}"; do
            echo "  - $(basename "$f")"
        done
        exit 1
    fi

    remaining=("${failed[@]}")
    ((round++))
done

if [[ ${#remaining[@]} -eq 0 ]]; then
    echo ""
    echo "All crates published successfully!"
else
    echo ""
    echo "WARNING: ${#remaining[@]} crates could not be published after $MAX_ROUNDS rounds."
    for f in "${remaining[@]}"; do
        echo "  - $(basename "$f")"
    done
    exit 1
fi
