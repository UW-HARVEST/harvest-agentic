#!/bin/bash

# visualize_all.py --watch schedules renders itself (in parallel, without
# waiting for slow ones); the loop only restarts it if it ever exits.
while true; do
  ./visualize_all.py --watch
  sleep 10
done
