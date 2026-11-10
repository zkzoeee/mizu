#!/usr/bin/env bash
# Timed shutdown safeguard, run on the pod under tmux.
#
# Ported unchanged in substance from wazabi r123. It exists because the cost
# ceiling must not depend on an agent staying attached, and because RunPod's
# own billing read came back with `recordCount: 0` for r120 and r123 alike —
# so spend is a deadline you enforce, not a number you can poll.
#
# It is deliberately dumb, and no API key is placed on a rented host. Halting
# the container releases the GPU and stops the compute charge; the pod still
# has to be terminated through the API to stop the disk charge, and a network
# volume keeps billing until it is deleted.
#
# The rate must be the rate the created pod reported, not the catalogue's:
# r114 printed a stale $1.00/hr while billing $1.29/hr.
set -euo pipefail

DEADLINE_MINUTES="${DEADLINE_MINUTES:?DEADLINE_MINUTES is required}"
RATE_PER_HOUR="${RATE_PER_HOUR:?RATE_PER_HOUR is required}"
CAP_USD="${CAP_USD:-unset}"
LOG=/workspace/results/watchdog.log
mkdir -p /workspace/results

COST=$(awk -v m="$DEADLINE_MINUTES" -v r="$RATE_PER_HOUR" 'BEGIN{printf "%.2f", m*r/60}')
{
  echo "watchdog armed $(date -u +%FT%TZ), deadline ${DEADLINE_MINUTES}m"
  echo "at \$${RATE_PER_HOUR}/hr that is \$${COST} against a cap of \$${CAP_USD}"
} | tee -a "$LOG"

sleep $(( DEADLINE_MINUTES * 60 ))

{
  echo "watchdog firing $(date -u +%FT%TZ): deadline reached"
  echo "results present at fire time:"
  ls -la /workspace/results 2>/dev/null || true
} | tee -a "$LOG"

sync
# Halt the container however this image allows: poweroff if the pod grants it,
# otherwise signal init directly.
poweroff 2>/dev/null || kill -TERM 1 2>/dev/null || kill -KILL 1
