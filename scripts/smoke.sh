#!/bin/bash
# Smoke driver for the repaint + input-region spike.
# Launches waynote, drives each spike case in sequence, and prints compositor
# surface state + visual expectations for manual verification.

set -euo pipefail

WAYNOTE_BIN="${WAYNOTE_BIN:-./target/debug/waynote}"
PAUSE_SEC=2
APP_ID="dev.mryll.waynote"

# Detect if running under Sway (SWAYSOCK or XDG_CURRENT_DESKTOP).
detect_compositor() {
    if [ -n "${SWAYSOCK:-}" ] || [ "${XDG_CURRENT_DESKTOP:-}" = "sway" ]; then
        echo "sway"
    else
        echo "hyprland"
    fi
}

# Print the current layer surface state from hyprctl or swaymsg.
print_surface_state() {
    local compositor="$1"
    echo ""
    echo ">>> Surface state ($compositor):"
    if [ "$compositor" = "sway" ]; then
        swaymsg -t get_tree 2>/dev/null | jq '.. | select(.type=="workspace") | {name, floating_nodes}' || echo "  (swaymsg unavailable or json parse failed)"
    else
        # Hyprland
        hyprctl layers -j 2>/dev/null | jq '.' || echo "  (hyprctl unavailable or json parse failed)"
    fi
}

# Invoke a gapplication action by name.
# Fails loudly on error — a dead app must not silently pass the smoke test.
invoke_action() {
    local action="$1"
    echo "  Invoking: $action"
    gdbus call \
        --session \
        --dest="dev.mryll.waynote" \
        --object-path="/dev/mryll/waynote" \
        --method="org.freedesktop.Application.ActivateAction" \
        "$action" \
        "[]" \
        "{}"
}

main() {
    COMPOSITOR=$(detect_compositor)
    echo "Detected compositor: $COMPOSITOR"
    echo ""

    # Launch the app in the background; fail loudly if the binary is missing.
    echo "[1/7] Launching waynote..."
    [ -x "$WAYNOTE_BIN" ] || { echo "ERROR: waynote binary not found or not executable: $WAYNOTE_BIN" >&2; exit 1; }
    "$WAYNOTE_BIN" &
    APP_PID=$!
    sleep 2  # Let the app start and register actions.

    # Define cases: (action_name, description_of_what_to_look_for)
    declare -a CASES=(
        "spike-reset:Seeded scene: yellow (#1) and green (#2) notes on Front layer, monitor 0."
        "spike-add:Added blue (#3) note. Three notes on Front; #3 should appear at (360, 60)."
        "spike-remove:Removed green (#2). Two notes remain (yellow #1, blue #3); no ghost frame."
        "spike-move:Moved yellow (#1) from (60,60) to (360,320). Positions: #1=(360,320), #3=(360,60)."
        "spike-move-across:Moved yellow (#1) to Desktop layer. Front now has only #3; #1 appears on Desktop."
        "spike-reset:Reset to seeded state (yellow #1, green #2, both Front)."
        "spike-batch:Moved all Front notes to Desktop. Front layer is now empty (fully click-through)."
    )

    for i in "${!CASES[@]}"; do
        IFS=':' read -r action desc <<< "${CASES[$i]}"
        step=$((i + 1))
        echo ""
        echo "[$step/7] Case: $action"
        echo "  Expected: $desc"
        invoke_action "$action"
        sleep 1  # Brief pause for repaint.
        print_surface_state "$COMPOSITOR"
        echo "  >>> Pause ${PAUSE_SEC}s. Visually confirm above."
        sleep "$PAUSE_SEC"
    done

    # Clean up.
    echo ""
    echo ">>> Closing app..."
    kill "$APP_PID" 2>/dev/null || true
    wait "$APP_PID" 2>/dev/null || true

    echo ""
    echo "=== Smoke test complete. Review the layer outputs and visual snapshots above. ==="
}

main "$@"
