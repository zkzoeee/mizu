#!/bin/bash
# All remaining slices of block 3, one process each so memory is bounded.
exec > /root/slices-all.log 2>&1
echo "begin=$(date -u +%FT%TZ)"
for i in $(seq 1 101); do
  SLICE=$i COUNT=103 /root/p2-slice.sh
done
echo "ALL_SLICES_DONE=$(date -u +%FT%TZ)"
ls /root/slices/*.bin | wc -l
