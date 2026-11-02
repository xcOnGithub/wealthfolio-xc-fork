#!/bin/sh
set -eu
# Host processes fetch the VM identity. Application containers must not inherit it.
iptables -C DOCKER-USER -d 169.254.169.254/32 -j REJECT 2>/dev/null ||
  iptables -I DOCKER-USER 1 -d 169.254.169.254/32 -j REJECT
