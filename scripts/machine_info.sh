#!/usr/bin/env bash
# Prints the machine details recorded alongside benchmark results.
set -euo pipefail

if [[ "$(uname)" == "Darwin" ]]; then
  echo "CPU: $(sysctl -n machdep.cpu.brand_string), $(sysctl -n hw.ncpu) cores"
  echo "Memory: $(( $(sysctl -n hw.memsize) / 1024 / 1024 / 1024 )) GiB"
  echo "OS: $(sw_vers -productName) $(sw_vers -productVersion) ($(uname -m))"
else
  echo "CPU: $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | xargs), $(nproc) cores"
  echo "Memory: $(( $(grep MemTotal /proc/meminfo | awk '{print $2}') / 1024 / 1024 )) GiB"
  echo "OS: $(. /etc/os-release && echo "$PRETTY_NAME") ($(uname -m))"
fi
echo "rustc: $(rustc --version)"
echo "redis-server: $(redis-server --version | awk '{print $3}')"
